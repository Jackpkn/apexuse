use crate::driver::{
    ActionRequest, AppSnapshot, DoctorStatus, OcrMode, OsDriver, Rect, UiElement,
};
use crate::json::JsonValue;
use crate::memory::{WorkflowMemory, WorkflowStep};
use crate::policy::{
    contains_secure_marker, valid_bundle_id, AuditLog, ConfirmationStore, Mode, PendingAction,
    PolicySettings, RateLimiter,
};
use crate::som::{format_mark_index, fuse_marks, supports_ax_press, VisualMark};
use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

pub struct ToolOutput {
    pub payload: JsonValue,
    pub image_png_b64: Option<String>,
    pub image_caption: Option<String>,
}

impl ToolOutput {
    pub fn json(payload: JsonValue) -> Self {
        Self {
            payload,
            image_png_b64: None,
            image_caption: None,
        }
    }
}

pub struct Gateway {
    driver: Box<dyn OsDriver>,
    settings: PolicySettings,
    audit: AuditLog,
    memory: WorkflowMemory,
    stop_file: PathBuf,
    rate_limiter: RateLimiter,
    confirmations: ConfirmationStore,
    epoch: u64,
    snapshot_app: Option<String>,
    snapshot_window: Option<i32>,
    snapshot_max_elements: i32,
    snapshot_max_depth: i32,
    snapshot_filtered: bool,
    snapshot_pid: Option<i64>,
    snapshot_windows_total: Option<usize>,
    snapshot_sheets: usize,
    snapshot_comparable: Option<String>,
    elements_by_ref: HashMap<String, UiElement>,
    marks_by_id: HashMap<String, VisualMark>,
}

impl Gateway {
    pub fn new(
        driver: Box<dyn OsDriver>,
        settings: PolicySettings,
        state_dir: PathBuf,
    ) -> Self {
        let audit = AuditLog::new(state_dir.join("logs/audit.jsonl"));
        let memory = WorkflowMemory::new(state_dir.join("memory/recipes.json"));
        let stop_file = state_dir.join("STOP");
        let rate_limiter = RateLimiter::new(settings.rate_limit_per_minute);
        let confirmations = ConfirmationStore::new(settings.confirmation_ttl_seconds);
        Self {
            driver,
            settings,
            audit,
            memory,
            stop_file,
            rate_limiter,
            confirmations,
            epoch: 0,
            snapshot_app: None,
            snapshot_window: None,
            snapshot_max_elements: 150,
            snapshot_max_depth: 12,
            snapshot_filtered: false,
            snapshot_pid: None,
            snapshot_windows_total: None,
            snapshot_sheets: 0,
            snapshot_comparable: None,
            elements_by_ref: HashMap::new(),
            marks_by_id: HashMap::new(),
        }
    }

    pub fn tick(&self) {
        self.driver.tick();
    }

    pub fn doctor(&self) -> JsonValue {
        match self.driver.doctor() {
            Ok(DoctorStatus {
                os,
                accessibility,
                screen_recording,
                note,
            }) => JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                ("os", JsonValue::String(os)),
                ("accessibility", JsonValue::Bool(accessibility)),
                ("screen_recording", JsonValue::Bool(screen_recording)),
                ("note", JsonValue::String(note)),
                (
                    "mode",
                    JsonValue::String(self.settings.mode.as_str().to_string()),
                ),
            ]),
            Err(e) => err_json(&e.code, &e.message),
        }
    }

    fn resolve_target(&self, raw: &str) -> Option<String> {
        if raw.eq_ignore_ascii_case("desktop") {
            return if self.settings.allowed_apps.iter().any(|a| a == "*") {
                Some("desktop".into())
            } else {
                None
            };
        }
        let mapped = match raw {
            "TextEdit" => "com.apple.TextEdit",
            "Calculator" => "com.apple.calculator",
            other => other,
        };
        if self.settings.permits_app(mapped) {
            Some(mapped.to_string())
        } else {
            None
        }
    }

    fn invalidate_snapshot(&mut self) {
        self.epoch += 1;
        self.snapshot_pid = None;
        self.snapshot_windows_total = None;
        self.snapshot_sheets = 0;
        self.snapshot_comparable = None;
        self.elements_by_ref.clear();
        self.marks_by_id.clear();
        self.confirmations.clear();
    }

    fn record_snapshot_meta(
        &mut self,
        snap: &AppSnapshot,
        window: Option<i32>,
        max_elements: i32,
        max_depth: i32,
        filtered: bool,
    ) {
        self.snapshot_app = Some(snap.bundle_id.clone());
        self.snapshot_window = window;
        self.snapshot_max_elements = max_elements;
        self.snapshot_max_depth = max_depth;
        self.snapshot_filtered = filtered;
        self.snapshot_pid = Some(snap.pid);
        self.snapshot_windows_total = Some(snap.windows_total);
        self.snapshot_sheets = snap.elements.iter().filter(|e| e.role == "Sheet").count();
        self.snapshot_comparable = Some(snap.comparable_text());
        for el in &snap.elements {
            if !el.ref_id.is_empty() {
                self.elements_by_ref.insert(el.ref_id.clone(), el.clone());
            }
        }
    }

    fn preflight_gate(&mut self, action: &str, mutating: bool) -> Option<ToolOutput> {
        if self.stop_file.exists() {
            return Some(ToolOutput::json(err_json("panic_stop", "STOP file present")));
        }
        if !self.rate_limiter.allow() {
            return Some(ToolOutput::json(err_json(
                "rate_limited",
                "Per-minute rate limit exceeded",
            )));
        }
        if mutating && self.settings.mode != Mode::Normal {
            let _ = self
                .audit
                .record(action, "deny", self.settings.mode.as_str());
            return Some(ToolOutput::json(err_json(
                self.settings.mode.as_str(),
                "Blocked by policy mode",
            )));
        }
        None
    }

    pub fn dispatch(&mut self, tool: &str, args: &JsonValue) -> ToolOutput {
        match tool {
            "inspect_app" => self.handle_inspect_app(args),
            "visual_inspect" => self.handle_visual_inspect(args),
            "screenshot" => self.handle_screenshot(args),
            "computer" => self.handle_computer(args),
            "desktop" => self.handle_desktop(args),
            "workflow_memory" => self.handle_memory(args),
            _ => ToolOutput::json(err_json("unknown_tool", "Unknown MCP tool")),
        }
    }

    fn check_frontmost_permitted(&self, front_bundle: &str) -> bool {
        front_bundle.is_empty() || self.settings.permits_app(front_bundle)
    }

    fn handle_inspect_app(&mut self, args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("inspect", false) {
            return blocked;
        }
        let Some(raw_app) = args.get("app").and_then(|v| v.as_str()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing 'app' bundle ID or 'desktop'"));
        };
        let Some(target) = self.resolve_target(raw_app) else {
            return ToolOutput::json(err_json("invalid_app", "Target is invalid, protected, or not permitted by policy"));
        };
        let window = match args.get("window") {
            None | Some(JsonValue::Null) => None,
            Some(v) => match v.as_i64() {
                Some(n) if n >= 0 => Some(n as i32),
                _ => return ToolOutput::json(err_json("invalid_argument", "'window' must be a non-negative integer")),
            },
        };
        let query = match args.get("query") {
            None | Some(JsonValue::Null) => None,
            Some(v) => match v.as_str() {
                Some(q) if !q.is_empty() && q.len() <= 120 && q.chars().all(|c| !c.is_control()) => Some(q),
                _ => return ToolOutput::json(err_json("invalid_argument", "Invalid 'query' string")),
            },
        };

        if self.audit.record("inspect", "allow", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        self.invalidate_snapshot();
        let snap = match self.driver.inspect_app(&target, window, 150, 12) {
            Ok(s) => s,
            Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
        };

        if !self.check_frontmost_permitted(&snap.frontmost_bundle_id) {
            let _ = self.audit.record("inspect", "deny", "blocked");
            return ToolOutput::json(err_json("invalid_app", "Frontmost application is protected"));
        }

        if let Some(reason) = snapshot_redaction_reason(&snap) {
            let _ = self.audit.record("inspect", "deny", "secure_field");
            return ToolOutput::json(JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                ("content", JsonValue::String("[redacted: secure field detected]".into())),
                ("redaction_reason", JsonValue::String(reason)),
            ]));
        }

        let outline = snap.render_outline(query);
        if contains_secure_marker(&outline) {
            return ToolOutput::json(JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                ("content", JsonValue::String("[redacted: secure field detected]".into())),
                ("redaction_reason", JsonValue::String("sensitive_text_marker".into())),
            ]));
        }

        self.record_snapshot_meta(&snap, window, 150, 12, query.is_some());

        let hint_bundle = if snap.bundle_id == "desktop" {
            &snap.frontmost_bundle_id
        } else {
            &snap.bundle_id
        };
        let hints = self
            .memory
            .lookup(hint_bundle, None)
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.to_json())
            .collect();

        ToolOutput::json(JsonValue::object(vec![
            ("ok", JsonValue::Bool(true)),
            ("app", JsonValue::String(snap.bundle_id)),
            ("frontmost_app", JsonValue::String(snap.frontmost_bundle_id)),
            ("pid", JsonValue::Number(snap.pid as f64)),
            ("windows_total", JsonValue::Number(snap.windows_total as f64)),
            ("content", JsonValue::String(outline)),
            ("workflow_hints", JsonValue::Array(hints)),
        ]))
    }

    fn validate_visual_snapshot(&self, target: &str, full: &AppSnapshot) -> Result<(), ToolOutput> {
        if !self.check_frontmost_permitted(&full.frontmost_bundle_id) {
            return Err(ToolOutput::json(err_json("invalid_app", "Frontmost application is protected")));
        }
        if (target != "desktop" && full.windows_total == 0)
            || full.has_secure_field
            || full.has_unverified_field
            || !full.safety_scan_complete
            || contains_secure_marker(&full.render_outline(None))
        {
            return Err(ToolOutput::json(err_json(
                "unsafe_or_incomplete_tree",
                "Visual capture refused: safety scan incomplete or window contains secure/unverified fields",
            )));
        }
        Ok(())
    }

    fn verify_visual_safety(&self, target: &str) -> Result<AppSnapshot, ToolOutput> {
        let full = match self.driver.inspect_app(target, None, 250, 14) {
            Ok(s) => s,
            Err(e) => return Err(ToolOutput::json(err_json(&e.code, &e.message))),
        };
        self.validate_visual_snapshot(target, &full)?;
        Ok(full)
    }

    fn handle_visual_inspect(&mut self, args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("screenshot", false) {
            return blocked;
        }
        let Some(raw_app) = args.get("app").and_then(|v| v.as_str()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing 'app' bundle ID or 'desktop'"));
        };
        let Some(target) = self.resolve_target(raw_app) else {
            return ToolOutput::json(err_json("invalid_app", "Target is invalid, protected, or not permitted"));
        };
        let window = match args.get("window") {
            None | Some(JsonValue::Null) => None,
            Some(v) => match v.as_i64() {
                Some(n) if n >= 0 => Some(n as i32),
                _ => return ToolOutput::json(err_json("invalid_argument", "'window' must be a non-negative integer")),
            },
        };
        let ocr_mode = match args.get("ocr").and_then(|v| v.as_str()) {
            None | Some("auto") | Some("fast") => OcrMode::Fast,
            Some("accurate") => OcrMode::Accurate,
            Some("off") | Some("none") => OcrMode::Off,
            Some(_) => return ToolOutput::json(err_json("invalid_argument", "'ocr' must be auto, fast, accurate, or off")),
        };
        let max_marks = match args.get("max_marks") {
            None | Some(JsonValue::Null) => 80usize,
            Some(v) => match v.as_i64() {
                Some(n) if (1..=150).contains(&n) => n as usize,
                _ => return ToolOutput::json(err_json("invalid_argument", "'max_marks' must be 1..=150")),
            },
        };
        if self.audit.record("screenshot", "allow", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        self.invalidate_snapshot();
        let (full, cap) = if target != "desktop" && window.is_none() {
            // Run AX safety walk and ScreenCaptureKit + Vision OCR concurrently.
            let driver = &*self.driver;
            let (ax_res, cap_res) = thread::scope(|s| {
                let cap_handle = s.spawn(|| driver.capture_and_ocr(&target, None, ocr_mode, false));
                let ax_res = driver.inspect_app(&target, None, 250, 14);
                let cap_res = match cap_handle.join() {
                    Ok(r) => r,
                    Err(_) => Err(crate::driver::DriverError::new("capture_failed", "Capture worker panicked")),
                };
                (ax_res, cap_res)
            });
            let full = match ax_res {
                Ok(s) => s,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            };
            if let Err(err_out) = self.validate_visual_snapshot(&target, &full) {
                return err_out;
            }
            let cap = match cap_res {
                Ok(c) => c,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            };
            (full, cap)
        } else {
            let full = match self.verify_visual_safety(&target) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let cap = match self.driver.capture_and_ocr(&full.bundle_id, window, ocr_mode, false) {
                Ok(c) => c,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            };
            (full, cap)
        };
        let scoped = if window.is_some() {
            match self.driver.inspect_app(&target, window, 150, 12) {
                Ok(s) => s,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            }
        } else {
            full.clone()
        };
        if !self.check_frontmost_permitted(&cap.frontmost_bundle_id) {
            return ToolOutput::json(err_json("invalid_app", "Frontmost application is protected"));
        }

        let win_elements: Vec<UiElement> = if window.is_none() && target != "desktop" && scoped.windows.len() > 1 {
            let cap_idx = scoped
                .windows
                .iter()
                .filter_map(|w| {
                    w.frame.as_ref().map(|f| {
                        let d = (f.x - cap.window_frame.x).abs()
                            + (f.y - cap.window_frame.y).abs()
                            + (f.w - cap.window_frame.w).abs()
                            + (f.h - cap.window_frame.h).abs();
                        (w.index, d)
                    })
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(idx, _)| idx)
                .unwrap_or(0);
            scoped
                .elements
                .iter()
                .filter(|e| e.window == cap_idx)
                .cloned()
                .collect()
        } else {
            scoped.elements.clone()
        };

        let marks = fuse_marks(&cap.window_frame, &win_elements, &cap.ocr_boxes, max_marks);
        let marks_overlay = JsonValue::Array(marks.iter().map(VisualMark::overlay_json).collect());

        let annotated_b64 = if marks.is_empty() && !cap.png_base64.is_empty() {
            cap.png_base64.clone()
        } else {
            let img_ref = cap.capture_id.as_deref().unwrap_or(cap.png_base64.as_str());
            match self.driver.render_som_png(
                img_ref,
                &marks_overlay.to_json_string(),
                &cap.window_frame,
            ) {
                Ok(b) => b,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            }
        };

        let (max_el, max_d) = if window.is_some() { (150, 12) } else { (250, 14) };
        self.record_snapshot_meta(&scoped, window, max_el, max_d, false);
        for m in &marks {
            self.marks_by_id.insert(m.id.clone(), m.clone());
        }

        let index_text = format_mark_index(&marks);
        let marks_json = JsonValue::Array(marks.iter().map(VisualMark::to_json).collect());
        let caption = if index_text.is_empty() {
            format!("Captured {} (no visual marks detected).", full.bundle_id)
        } else {
            format!(
                "Set-of-Mark visual index for {} ({} marks):\n{}",
                full.bundle_id,
                marks.len(),
                index_text
            )
        };

        ToolOutput {
            payload: JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                ("app", JsonValue::String(full.bundle_id)),
                ("frontmost_app", JsonValue::String(cap.frontmost_bundle_id)),
                ("window_id", JsonValue::Number(cap.window_id as f64)),
                ("marks_count", JsonValue::Number(marks.len() as f64)),
                ("marks", marks_json),
                ("index_text", JsonValue::String(index_text)),
            ]),
            image_png_b64: Some(annotated_b64),
            image_caption: Some(caption),
        }
    }

    fn handle_screenshot(&mut self, args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("screenshot", false) {
            return blocked;
        }
        let Some(raw_app) = args.get("app").and_then(|v| v.as_str()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing 'app' bundle ID or 'desktop'"));
        };
        let Some(target) = self.resolve_target(raw_app) else {
            return ToolOutput::json(err_json("invalid_app", "Target is invalid, protected, or not permitted"));
        };
        if self.audit.record("screenshot", "allow", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        self.invalidate_snapshot();
        let full = match self.verify_visual_safety(&target) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let cap = match self.driver.capture_and_ocr(&full.bundle_id, None, OcrMode::Off, true) {
            Ok(c) => c,
            Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
        };

        let caption = format!(
            "Captured {} ({}x{}).",
            full.bundle_id, cap.image_width, cap.image_height
        );
        ToolOutput {
            payload: JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                ("app", JsonValue::String(full.bundle_id)),
                ("window_id", JsonValue::Number(cap.window_id as f64)),
            ]),
            image_png_b64: Some(cap.png_base64),
            image_caption: Some(caption),
        }
    }

    fn resolve_point(&self, args: &JsonValue, ref_key: &str, mark_key: &str, x_key: &str, y_key: &str) -> Result<(f64, f64, bool, String), ToolOutput> {
        if let Some(mid) = args.get(mark_key).and_then(|v| v.as_str()) {
            let Some(mark) = self.marks_by_id.get(mid) else {
                return Err(ToolOutput::json(err_json("stale_ref", "Unknown or stale visual mark ID")));
            };
            return Ok((mark.center_x, mark.center_y, mark.prefer_ax, format!("mark {mid} ({} \"{}\")", mark.role, mark.label)));
        }
        if let Some(rid) = args.get(ref_key).and_then(|v| v.as_str()) {
            let Some(el) = self.elements_by_ref.get(rid) else {
                return Err(ToolOutput::json(err_json("stale_ref", "Unknown or stale element ref")));
            };
            let Some(ref f) = el.frame else {
                return Err(ToolOutput::json(err_json("target_identity_unverified", "Element lacks coordinates")));
            };
            let prefer_ax = supports_ax_press(el);
            let label = if el.label.is_empty() { el.role.clone() } else { el.label.clone() };
            return Ok((f.x + f.w * 0.5, f.y + f.h * 0.5, prefer_ax, format!("{} '{}' ({rid})", el.role, label)));
        }
        if let (Some(x), Some(y)) = (args.get(x_key).and_then(|v| v.as_f64()), args.get(y_key).and_then(|v| v.as_f64())) {
            if !x.is_finite() || !y.is_finite() || x.abs() > 100000.0 || y.abs() > 100000.0 {
                return Err(ToolOutput::json(err_json("invalid_argument", "Coordinate out of bounds")));
            }
            return Ok((x, y, false, format!("({x:.0}, {y:.0})")));
        }
        Err(ToolOutput::json(err_json("invalid_argument", "Missing target ref, mark, or coordinates")))
    }

    fn request_pointer_click(&mut self, action_name: &str, sub_args: &JsonValue, click_count: u8, button: &str, allow_coords: bool) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate(action_name, true) {
            return blocked;
        }
        let Some(app) = self.snapshot_app.clone() else {
            return ToolOutput::json(err_json("stale_ref", "Inspect or visual_inspect an app or 'desktop' first"));
        };
        if self.snapshot_filtered {
            return ToolOutput::json(err_json("invalid_state", "Visual coordinate clicks require an unfiltered snapshot"));
        }

        if sub_args.get("mark").is_none() && sub_args.get("x").is_none() && sub_args.get("y").is_none() && sub_args.get("ref").is_some() {
            let rid = sub_args.get("ref").and_then(|v| v.as_str()).unwrap_or("");
            let Some(el) = self.elements_by_ref.get(rid).cloned() else {
                return ToolOutput::json(err_json("stale_ref", "Unknown or stale element ref"));
            };
            if el.enabled == Some(false) {
                return ToolOutput::json(err_json("not_implemented", "Element is disabled"));
            }
            if el.frame.is_none() {
                return ToolOutput::json(err_json("target_identity_unverified", "Element lacks screen coordinates"));
            }
            let label = if el.label.is_empty() { el.role.clone() } else { el.label.clone() };
            let summary = format!("{action_name} {} '{}' in {app}. Ask the user before confirming.", el.role, label);
            let pending = PendingAction::ClickRef {
                epoch: self.epoch,
                ref_id: rid.to_string(),
                label,
                click_count,
                button: button.to_string(),
            };
            return self.dispatch_pending(action_name, pending, &summary);
        }

        let (x, y, prefer_ax, target_desc) = match self.resolve_point(sub_args, "ref", "mark", "x", "y") {
            Ok(res) => res,
            Err(e) => return e,
        };
        if (sub_args.get("x").is_some() || sub_args.get("y").is_some()) && !allow_coords {
            return ToolOutput::json(err_json("invalid_argument", "Use desktop/visual_click or provide ref/mark"));
        }
        let mark_id = sub_args.get("mark").and_then(|v| v.as_str()).map(str::to_string);
        let summary = format!("{action_name} {target_desc} in {app}. Ask the user before confirming.");
        let pending = PendingAction::VisualClick {
            epoch: self.epoch,
            mark_id,
            x,
            y,
            prefer_ax: prefer_ax && click_count == 1 && button == "left",
            click_count,
            button: button.to_string(),
        };
        self.dispatch_pending(action_name, pending, &summary)
    }

    fn request_hover(&mut self, sub_args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("hover", true) {
            return blocked;
        }
        let Some(app) = self.snapshot_app.clone() else {
            return ToolOutput::json(err_json("stale_ref", "Inspect first"));
        };
        let (x, y, _, desc) = match self.resolve_point(sub_args, "ref", "mark", "x", "y") {
            Ok(r) => r,
            Err(e) => return e,
        };
        self.dispatch_pending(
            "hover",
            PendingAction::Hover { epoch: self.epoch, x, y },
            &format!("Hover over {desc} in {app}. Ask the user before confirming."),
        )
    }

    fn request_drag(&mut self, sub_args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("drag", true) {
            return blocked;
        }
        let Some(app) = self.snapshot_app.clone() else {
            return ToolOutput::json(err_json("stale_ref", "Inspect first"));
        };
        let (x1, y1, _, desc1) = match self.resolve_point(sub_args, "from_ref", "from_mark", "x1", "y1") {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (x2, y2, _, desc2) = match self.resolve_point(sub_args, "to_ref", "to_mark", "x2", "y2") {
            Ok(r) => r,
            Err(e) => return e,
        };
        self.dispatch_pending(
            "drag",
            PendingAction::Drag {
                epoch: self.epoch,
                x1,
                y1,
                x2,
                y2,
            },
            &format!("Drag from {desc1} to {desc2} in {app}. Ask the user before confirming."),
        )
    }

    fn handle_desktop(&mut self, args: &JsonValue) -> ToolOutput {
        let Some(action) = args.get("action").and_then(|v| v.as_str()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing 'action'"));
        };
        let sub_args = args.get("arguments").unwrap_or(&JsonValue::Null);

        match action {
            "list_applications" => {
                if let Some(blocked) = self.preflight_gate("list_applications", false) {
                    return blocked;
                }
                if self.audit.record("list_applications", "allow", "requested").is_err() {
                    return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
                }
                match self.driver.list_apps() {
                    Ok(apps) => {
                        let permitted: Vec<JsonValue> = apps
                            .into_iter()
                            .filter(|a| self.settings.permits_app(&a.bundle_id))
                            .map(|a| {
                                JsonValue::object(vec![
                                    ("bundle_id", JsonValue::String(a.bundle_id)),
                                    ("pid", JsonValue::Number(a.pid as f64)),
                                    ("active", JsonValue::Bool(a.active)),
                                ])
                            })
                            .collect();
                        ToolOutput::json(JsonValue::object(vec![
                            ("ok", JsonValue::Bool(true)),
                            ("apps", JsonValue::Array(permitted)),
                        ]))
                    }
                    Err(e) => ToolOutput::json(err_json(&e.code, &e.message)),
                }
            }
            "activate_application" | "launch_application" => {
                if let Some(blocked) = self.preflight_gate(action, true) {
                    return blocked;
                }
                let Some(raw_app) = sub_args.get("app").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing arguments.app"));
                };
                if raw_app.eq_ignore_ascii_case("desktop") {
                    return ToolOutput::json(err_json("invalid_app", "Specify an application bundle ID"));
                }
                let Some(bundle) = self.resolve_target(raw_app) else {
                    return ToolOutput::json(err_json("invalid_app", "App is invalid, protected, or not permitted"));
                };
                if self.audit.record(action, "allow", "requested").is_err() {
                    return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
                }
                self.invalidate_snapshot();
                let res = if action == "activate_application" {
                    self.driver.activate_app(&bundle)
                } else {
                    self.driver.launch_app(&bundle)
                };
                match res {
                    Ok(()) => ToolOutput::json(JsonValue::object(vec![
                        ("ok", JsonValue::Bool(true)),
                        ("app", JsonValue::String(bundle)),
                        ("hint", JsonValue::String("Inspect the app to verify its updated state.".into())),
                    ])),
                    Err(e) => ToolOutput::json(err_json(&e.code, &e.message)),
                }
            }
            "open_url" => {
                if let Some(blocked) = self.preflight_gate("open_url", true) {
                    return blocked;
                }
                let Some(url) = sub_args.get("url").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing arguments.url"));
                };
                if url.is_empty()
                    || url.len() > 2048
                    || !(url.starts_with("https://") || url.starts_with("http://"))
                    || url.chars().any(|c| c.is_control() || c.is_whitespace())
                {
                    return ToolOutput::json(err_json("invalid_argument", "Invalid http/https URL"));
                }
                let raw_app = sub_args
                    .get("app")
                    .and_then(|v| v.as_str())
                    .unwrap_or("com.google.Chrome");
                let Some(bundle) = self.resolve_target(raw_app) else {
                    return ToolOutput::json(err_json("invalid_app", "App is invalid, protected, or not permitted"));
                };
                if self.audit.record("open_url", "allow", "requested").is_err() {
                    return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
                }
                self.invalidate_snapshot();
                match self.driver.open_url(&bundle, url) {
                    Ok(()) => ToolOutput::json(JsonValue::object(vec![
                        ("ok", JsonValue::Bool(true)),
                        ("app", JsonValue::String(bundle)),
                        ("url", JsonValue::String(url.into())),
                    ])),
                    Err(e) => ToolOutput::json(err_json(&e.code, &e.message)),
                }
            }
            "visual_click" => self.request_pointer_click("visual_click", sub_args, 1, "left", true),
            "double_click" => self.request_pointer_click("double_click", sub_args, 2, "left", true),
            "right_click" => self.request_pointer_click("right_click", sub_args, 1, "right", true),
            "hover" => self.request_hover(sub_args),
            "drag" => self.request_drag(sub_args),
            _ => ToolOutput::json(err_json("not_implemented", "Desktop action not supported")),
        }
    }

    fn handle_computer(&mut self, args: &JsonValue) -> ToolOutput {
        let Some(action) = args.get("action").and_then(|v| v.as_str()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing 'action'"));
        };
        let sub_args = args.get("arguments").unwrap_or(&JsonValue::Null);

        match action {
            "inspect" => self.handle_inspect_app(sub_args),
            "click" => self.request_pointer_click("click", sub_args, 1, "left", false),
            "double_click" => self.request_pointer_click("double_click", sub_args, 2, "left", false),
            "right_click" => self.request_pointer_click("right_click", sub_args, 1, "right", false),
            "hover" => self.request_hover(sub_args),
            "drag" => self.request_drag(sub_args),
            "type" => {
                if let Some(blocked) = self.preflight_gate("type", true) {
                    return blocked;
                }
                let Some(app) = self.snapshot_app.clone() else {
                    return ToolOutput::json(err_json("stale_ref", "Inspect an app first"));
                };
                let Some(text) = sub_args.get("text").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing arguments.text"));
                };
                let mode = sub_args.get("mode").and_then(|v| v.as_str()).unwrap_or("insert");
                if !matches!(mode, "insert" | "replace") {
                    return ToolOutput::json(err_json("invalid_argument", "mode must be 'insert' or 'replace'"));
                }
                let max_len = 2000;
                if text.is_empty() || text.chars().count() > max_len {
                    return ToolOutput::json(err_json("invalid_argument", "Text length out of allowed range"));
                }
                if !text.chars().all(|c| !c.is_control() || matches!(c, '\n' | '\t')) {
                    return ToolOutput::json(err_json("invalid_argument", "Control characters are not allowed"));
                }

                let (ref_id, cx, cy, click_first) = if let Some(rid) = sub_args.get("ref").and_then(|v| v.as_str()) {
                    let Some(el) = self.elements_by_ref.get(rid) else {
                        return ToolOutput::json(err_json("stale_ref", "Unknown or stale element ref"));
                    };
                    let is_native_text = matches!(
                        el.role.as_str(),
                        "TextArea" | "TextField" | "SearchField" | "ComboBox"
                    );
                    if mode == "replace" && !is_native_text {
                        return ToolOutput::json(err_json("target_identity_unverified", "Replace mode requires a verified text field"));
                    }
                    let Some(ref f) = el.frame else {
                        return ToolOutput::json(err_json("target_identity_unverified", "Target lacks bounding frame"));
                    };
                    (Some(rid.to_string()), f.x + f.w * 0.5, f.y + f.h * 0.5, !is_native_text)
                } else if let Some(mid) = sub_args.get("mark").and_then(|v| v.as_str()) {
                    let Some(mark) = self.marks_by_id.get(mid) else {
                        return ToolOutput::json(err_json("stale_ref", "Unknown or stale visual mark ID"));
                    };
                    if mode == "replace" {
                        return ToolOutput::json(err_json("invalid_argument", "Replace mode requires a verified Accessibility ref"));
                    }
                    (mark.ax_ref.clone(), mark.center_x, mark.center_y, true)
                } else {
                    return ToolOutput::json(err_json("invalid_argument", "Provide arguments.ref or arguments.mark"));
                };

                let summary = if mode == "insert" {
                    format!("Type {text:?} into {app}. Ask the user before confirming.")
                } else {
                    format!("Replace text field in {app} with {text:?}. Ask the user before confirming.")
                };
                self.dispatch_pending(
                    "type",
                    PendingAction::TypeText {
                        epoch: self.epoch,
                        ref_id,
                        x: cx,
                        y: cy,
                        text: text.to_string(),
                        mode: mode.to_string(),
                        click_first,
                    },
                    &summary,
                )
            }
            "press_key" => {
                if let Some(blocked) = self.preflight_gate("press_key", true) {
                    return blocked;
                }
                let Some(app) = self.snapshot_app.clone() else {
                    return ToolOutput::json(err_json("stale_ref", "Inspect an app first"));
                };
                if self.snapshot_filtered {
                    return ToolOutput::json(err_json("invalid_state", "press_key requires an unfiltered snapshot"));
                }
                let Some(key) = sub_args.get("key").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing arguments.key"));
                };
                let mods: Vec<String> = sub_args
                    .get("modifiers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_ascii_lowercase()))
                            .collect()
                    })
                    .unwrap_or_default();
                if !valid_key_request(key, &mods) {
                    return ToolOutput::json(err_json("invalid_argument", "Unsupported key or blocked paste shortcut"));
                }
                let chord = if mods.is_empty() {
                    key.to_string()
                } else {
                    format!("{}+{}", mods.join("+"), key)
                };
                let summary = format!("Press {chord} in {app}. Ask the user before confirming.");
                self.dispatch_pending(
                    "press_key",
                    PendingAction::PressKey {
                        epoch: self.epoch,
                        key: key.to_string(),
                        modifiers: mods,
                    },
                    &summary,
                )
            }
            "scroll" => {
                if let Some(blocked) = self.preflight_gate("scroll", true) {
                    return blocked;
                }
                let Some(app) = self.snapshot_app.clone() else {
                    return ToolOutput::json(err_json("stale_ref", "Inspect an app first"));
                };
                if self.snapshot_filtered {
                    return ToolOutput::json(err_json("invalid_state", "scroll requires an unfiltered snapshot"));
                }
                let Some(direction) = sub_args.get("direction").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing arguments.direction"));
                };
                if !matches!(direction, "up" | "down" | "left" | "right") {
                    return ToolOutput::json(err_json("invalid_argument", "Invalid scroll direction"));
                }
                let amount = sub_args.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
                if !(1..=100).contains(&amount) {
                    return ToolOutput::json(err_json("invalid_argument", "Scroll amount must be 1..=100"));
                }
                let summary = format!("Scroll {direction} {amount} lines in {app}. Ask the user before confirming.");
                self.dispatch_pending(
                    "scroll",
                    PendingAction::Scroll {
                        epoch: self.epoch,
                        direction: direction.to_string(),
                        amount: amount as i32,
                    },
                    &summary,
                )
            }
            "run_actions" => self.handle_run_actions(sub_args),
            "wait_for" => self.handle_wait_for(sub_args),
            "confirm" => {
                if let Some(blocked) = self.preflight_gate("confirm", true) {
                    return blocked;
                }
                let Some(token) = sub_args.get("token").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing arguments.token"));
                };
                let Some(approved) = self.confirmations.consume(token) else {
                    return ToolOutput::json(err_json("invalid_confirmation", "Invalid or expired confirmation token"));
                };
                self.execute_action(approved, "confirmed")
            }
            _ => ToolOutput::json(err_json("not_implemented", "Operation disabled")),
        }
    }

    fn dispatch_pending(&mut self, action_name: &str, action: PendingAction, summary: &str) -> ToolOutput {
        if !self.settings.require_confirmation {
            return self.execute_action(action, "direct");
        }
        if self.audit.record(action_name, "confirm", "confirmation_required").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }
        let token = self.confirmations.issue(action);
        ToolOutput::json(confirmation_json(&token, summary))
    }

    fn fresh_has_element_near(
        &self,
        fresh: &AppSnapshot,
        x: f64,
        y: f64,
        expected_role: Option<&str>,
        expected_label: Option<&str>,
    ) -> bool {
        fresh.elements.iter().any(|el| {
            let Some(ref f) = el.frame else {
                return false;
            };
            let cx = f.x + f.w * 0.5;
            let cy = f.y + f.h * 0.5;
            let near = (cx - x).abs() <= 24.0 && (cy - y).abs() <= 24.0;
            if !near {
                return false;
            }
            if let Some(role) = expected_role {
                if el.role != role {
                    return false;
                }
            }
            if let Some(label) = expected_label {
                if !label.is_empty() && !el.label.is_empty() && el.label != label {
                    return false;
                }
            }
            true
        })
    }

    fn snapshot_matches_action(&self, fresh: &AppSnapshot, action: &PendingAction) -> bool {
        if self.snapshot_pid.is_some_and(|p| p != fresh.pid) {
            return false;
        }
        let fresh_sheets = fresh.elements.iter().filter(|e| e.role == "Sheet").count();
        if fresh_sheets > self.snapshot_sheets {
            return false;
        }
        match action {
            PendingAction::ClickRef { ref_id, label, .. } => {
                let Some(orig) = self.elements_by_ref.get(ref_id) else {
                    return false;
                };
                let Some(ref f) = orig.frame else {
                    return false;
                };
                self.fresh_has_element_near(
                    fresh,
                    f.x + f.w * 0.5,
                    f.y + f.h * 0.5,
                    Some(&orig.role),
                    Some(label.as_str()),
                )
            }
            PendingAction::VisualClick { mark_id, x, y, .. } => {
                if let Some(mid) = mark_id {
                    if let Some(mark) = self.marks_by_id.get(mid) {
                        if mark.ax_ref.is_some() {
                            return self.fresh_has_element_near(
                                fresh,
                                *x,
                                *y,
                                Some(&mark.role),
                                Some(&mark.label),
                            );
                        }
                    }
                }
                true
            }
            PendingAction::TypeText { ref_id, x, y, click_first, .. } => {
                if ref_id.is_some() {
                    if *click_first {
                        return self.fresh_has_element_near(fresh, *x, *y, None, None);
                    }
                    return fresh.elements.iter().any(|el| {
                        let Some(ref f) = el.frame else {
                            return false;
                        };
                        matches!(
                            el.role.as_str(),
                            "TextArea" | "TextField" | "SearchField" | "ComboBox"
                        ) && ((f.x + f.w * 0.5) - x).abs() <= 24.0
                            && ((f.y + f.h * 0.5) - y).abs() <= 24.0
                    });
                }
                true
            }
            _ => true,
        }
    }

    fn execute_action(&mut self, action: PendingAction, policy_label: &str) -> ToolOutput {
        let Some(app) = self.snapshot_app.clone() else {
            return ToolOutput::json(err_json("stale_ref", "No active snapshot"));
        };
        let action_epoch = match &action {
            PendingAction::ClickRef { epoch, .. }
            | PendingAction::VisualClick { epoch, .. }
            | PendingAction::Hover { epoch, .. }
            | PendingAction::Drag { epoch, .. }
            | PendingAction::TypeText { epoch, .. }
            | PendingAction::PressKey { epoch, .. }
            | PendingAction::Scroll { epoch, .. } => *epoch,
        };
        if action_epoch != self.epoch {
            return ToolOutput::json(err_json("stale_ref", "Snapshot changed before confirmation"));
        }

        let fast_direct = policy_label == "direct" && self.driver.supports_fast_point_verify();
        if fast_direct {
            let point_check = match &action {
                PendingAction::ClickRef { ref_id, .. } => self
                    .elements_by_ref
                    .get(ref_id)
                    .and_then(|el| el.frame.as_ref())
                    .map(|f| vec![(f.x + f.w * 0.5, f.y + f.h * 0.5)]),
                PendingAction::VisualClick { x, y, .. }
                | PendingAction::Hover { x, y, .. }
                | PendingAction::TypeText { x, y, .. } => Some(vec![(*x, *y)]),
                PendingAction::Drag { x1, y1, x2, y2, .. } => Some(vec![(*x1, *y1), (*x2, *y2)]),
                PendingAction::PressKey { .. } | PendingAction::Scroll { .. } => Some(Vec::new()),
            };
            let Some(pts) = point_check else {
                self.invalidate_snapshot();
                return ToolOutput::json(err_json("stale_ref", "Element ref or frame missing"));
            };
            for (px, py) in pts {
                match self.driver.verify_point_safe(&app, px, py) {
                    Ok(true) => {}
                    Ok(false) => {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json(
                            "unsafe_target_field",
                            "Target point became unsafe or left window bounds",
                        ));
                    }
                    Err(e) => {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json(&e.code, &e.message));
                    }
                }
            }
        } else {
            let fresh = match self.driver.inspect_app(
                &app,
                self.snapshot_window,
                self.snapshot_max_elements,
                self.snapshot_max_depth,
            ) {
                Ok(s) => s,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            };
            if !self.check_frontmost_permitted(&fresh.frontmost_bundle_id)
                || snapshot_redaction_reason(&fresh).is_some()
                || !self.snapshot_matches_action(&fresh, &action)
            {
                self.invalidate_snapshot();
                return ToolOutput::json(err_json(
                    "stale_ref",
                    "Target UI changed or became unsafe before input; inspect again",
                ));
            }
        }

        let action_name = match &action {
            PendingAction::ClickRef { click_count: 2, .. }
            | PendingAction::VisualClick { click_count: 2, .. } => "double_click",
            PendingAction::ClickRef { button, .. }
            | PendingAction::VisualClick { button, .. }
                if button == "right" =>
            {
                "right_click"
            }
            PendingAction::ClickRef { .. } => "click",
            PendingAction::VisualClick { .. } => "visual_click",
            PendingAction::Hover { .. } => "hover",
            PendingAction::Drag { .. } => "drag",
            PendingAction::TypeText { .. } => "type",
            PendingAction::PressKey { .. } => "press_key",
            PendingAction::Scroll { .. } => "scroll",
        };
        if self.audit.record(action_name, policy_label, "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        let driver_req = match action {
            PendingAction::ClickRef {
                ref_id,
                click_count,
                button,
                ..
            } => {
                let Some(el) = self.elements_by_ref.get(&ref_id) else {
                    self.invalidate_snapshot();
                    return ToolOutput::json(err_json("stale_ref", "Element ref missing"));
                };
                let Some(ref f) = el.frame else {
                    self.invalidate_snapshot();
                    return ToolOutput::json(err_json("target_identity_unverified", "Missing frame"));
                };
                let prefer_ax = click_count == 1 && button == "left" && supports_ax_press(el);
                ActionRequest::ClickPoint {
                    bundle_id: app.clone(),
                    x: f.x + f.w * 0.5,
                    y: f.y + f.h * 0.5,
                    prefer_ax,
                    click_count,
                    button,
                }
            }
            PendingAction::VisualClick {
                x,
                y,
                prefer_ax,
                click_count,
                button,
                ..
            } => ActionRequest::ClickPoint {
                bundle_id: app.clone(),
                x,
                y,
                prefer_ax,
                click_count,
                button,
            },
            PendingAction::Hover { x, y, .. } => ActionRequest::HoverPoint {
                bundle_id: app.clone(),
                x,
                y,
            },
            PendingAction::Drag { x1, y1, x2, y2, .. } => ActionRequest::DragPoints {
                bundle_id: app.clone(),
                x1,
                y1,
                x2,
                y2,
            },
            PendingAction::TypeText {
                x,
                y,
                text,
                mode,
                click_first,
                ..
            } => ActionRequest::TypePoint {
                bundle_id: app.clone(),
                x,
                y,
                text,
                mode,
                click_first,
            },
            PendingAction::PressKey { key, modifiers, .. } => ActionRequest::PressKey {
                bundle_id: app.clone(),
                key,
                modifiers,
            },
            PendingAction::Scroll {
                direction, amount, ..
            } => ActionRequest::Scroll {
                bundle_id: app.clone(),
                direction,
                amount,
            },
        };

        self.invalidate_snapshot();
        match self.driver.perform_action(&driver_req) {
            Ok(res) => ToolOutput::json(JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                ("policy", JsonValue::String(policy_label.into())),
                ("completed", JsonValue::Bool(false)),
                ("method", JsonValue::String(res.method)),
                (
                    "hint",
                    JsonValue::String(
                        "Verify the resulting state via inspect_app or visual_inspect.".into(),
                    ),
                ),
            ])),
            Err(e) => ToolOutput::json(JsonValue::object(vec![
                ("ok", JsonValue::Bool(false)),
                ("code", JsonValue::String(e.code)),
                ("message", JsonValue::String(e.message)),
                ("completed", JsonValue::Bool(false)),
                (
                    "hint",
                    JsonValue::String("Inspect state before retrying.".into()),
                ),
            ])),
        }
    }

    fn handle_memory(&mut self, args: &JsonValue) -> ToolOutput {
        let Some(action) = args.get("action").and_then(|v| v.as_str()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing 'action'"));
        };
        match action {
            "list" => match self.memory.load_all() {
                Ok(all) => ToolOutput::json(JsonValue::object(vec![
                    ("ok", JsonValue::Bool(true)),
                    (
                        "recipes",
                        JsonValue::Array(all.iter().map(|r| r.to_json()).collect()),
                    ),
                ])),
                Err(e) => ToolOutput::json(err_json("memory_error", &e)),
            },
            "lookup" => {
                let Some(raw_app) = args.get("app").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing 'app'"));
                };
                let Some(bundle) = self.resolve_target(raw_app) else {
                    return ToolOutput::json(err_json("invalid_app", "Invalid or protected app"));
                };
                let q = args.get("query").and_then(|v| v.as_str());
                match self.memory.lookup(&bundle, q) {
                    Ok(found) => ToolOutput::json(JsonValue::object(vec![
                        ("ok", JsonValue::Bool(true)),
                        (
                            "recipes",
                            JsonValue::Array(found.iter().map(|r| r.to_json()).collect()),
                        ),
                    ])),
                    Err(e) => ToolOutput::json(err_json("memory_error", &e)),
                }
            }
            "record" => {
                if let Some(blocked) = self.preflight_gate("memory_record", true) {
                    return blocked;
                }
                let Some(raw_app) = args.get("app").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing 'app'"));
                };
                let Some(bundle) = self.resolve_target(raw_app) else {
                    return ToolOutput::json(err_json("invalid_app", "Invalid or protected app"));
                };
                let Some(goal) = args.get("goal").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing 'goal'"));
                };
                let Some(arr) = args.get("steps").and_then(|v| v.as_array()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing 'steps' array"));
                };
                let steps: Vec<WorkflowStep> = arr
                    .iter()
                    .map(|item| WorkflowStep {
                        action: item.get("action").and_then(|x| x.as_str()).unwrap_or("").into(),
                        target_role: item.get("target_role").and_then(|x| x.as_str()).unwrap_or("").into(),
                        target_label: item.get("target_label").and_then(|x| x.as_str()).unwrap_or("").into(),
                        note: item.get("note").and_then(|x| x.as_str()).unwrap_or("").into(),
                    })
                    .collect();
                match self.memory.record(&bundle, goal, steps) {
                    Ok(rec) => ToolOutput::json(JsonValue::object(vec![
                        ("ok", JsonValue::Bool(true)),
                        ("recipe", rec.to_json()),
                    ])),
                    Err(e) => ToolOutput::json(err_json("memory_error", &e)),
                }
            }
            "clear" => {
                if let Some(blocked) = self.preflight_gate("memory_clear", true) {
                    return blocked;
                }
                let Some(raw_app) = args.get("app").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing 'app'"));
                };
                if !valid_bundle_id(raw_app) {
                    return ToolOutput::json(err_json("invalid_app", "Invalid bundle ID"));
                }
                match self.memory.clear_app(raw_app) {
                    Ok(removed) => ToolOutput::json(JsonValue::object(vec![
                        ("ok", JsonValue::Bool(true)),
                        ("removed", JsonValue::Number(removed as f64)),
                    ])),
                    Err(e) => ToolOutput::json(err_json("memory_error", &e)),
                }
            }
            "replay" => self.handle_memory_replay(args),
            _ => ToolOutput::json(err_json("invalid_argument", "Unknown memory action")),
        }
    }

    fn handle_run_actions(&mut self, sub_args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("run_actions", true) {
            return blocked;
        }
        if self.settings.require_confirmation {
            return ToolOutput::json(err_json(
                "confirmation_required",
                "Batch run_actions requires confirmation: direct policy",
            ));
        }
        let Some(steps) = sub_args.get("steps").and_then(|v| v.as_array()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing arguments.steps array"));
        };
        if steps.is_empty() || steps.len() > 12 {
            return ToolOutput::json(err_json("invalid_argument", "steps must contain 1..=12 items"));
        }
        let inspect_after = sub_args
            .get("inspect_after")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut cur_app = if let Some(raw_app) = sub_args.get("app").and_then(|v| v.as_str()) {
            let Some(resolved) = self.resolve_target(raw_app) else {
                return ToolOutput::json(err_json("invalid_app", "Target is invalid, protected, or not permitted"));
            };
            resolved
        } else if let Some(existing) = self.snapshot_app.clone() {
            existing
        } else {
            return ToolOutput::json(err_json("stale_ref", "Inspect an app or pass arguments.app first"));
        };

        if self.audit.record("run_actions", "direct", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        let elements_copy = self.elements_by_ref.clone();
        let marks_copy = self.marks_by_id.clone();
        let mut step_results = Vec::with_capacity(steps.len());

        for (idx, step) in steps.iter().enumerate() {
            if self.stop_file.exists() {
                self.invalidate_snapshot();
                return ToolOutput::json(err_json("panic_stop", "STOP file present"));
            }
            let Some(act) = step.get("action").and_then(|v| v.as_str()) else {
                self.invalidate_snapshot();
                return ToolOutput::json(err_json("invalid_argument", &format!("Step {idx} missing action")));
            };

            match act {
                "sleep_ms" => {
                    let ms = step.get("ms").and_then(|v| v.as_i64()).unwrap_or(200).clamp(10, 3000) as u64;
                    thread::sleep(Duration::from_millis(ms));
                    self.driver.tick();
                    step_results.push(JsonValue::object(vec![
                        ("step", JsonValue::Number(idx as f64)),
                        ("action", JsonValue::String("sleep_ms".into())),
                        ("ok", JsonValue::Bool(true)),
                    ]));
                }
                "activate_application" => {
                    let Some(raw) = step.get("app").and_then(|v| v.as_str()) else {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", &format!("Step {idx} missing app")));
                    };
                    let Some(bundle) = self.resolve_target(raw) else {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_app", "Target is invalid, protected, or not permitted"));
                    };
                    if let Err(e) = self.driver.activate_app(&bundle) {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json(&e.code, &e.message));
                    }
                    cur_app = bundle.clone();
                    step_results.push(JsonValue::object(vec![
                        ("step", JsonValue::Number(idx as f64)),
                        ("action", JsonValue::String("activate_application".into())),
                        ("app", JsonValue::String(bundle)),
                        ("ok", JsonValue::Bool(true)),
                    ]));
                }
                "click" | "double_click" | "right_click" | "hover" => {
                    let (x, y, prefer_ax) = match resolve_batch_point(step, &elements_copy, &marks_copy) {
                        Ok(v) => v,
                        Err(e) => {
                            self.invalidate_snapshot();
                            return e;
                        }
                    };
                    match self.driver.verify_point_safe(&cur_app, x, y) {
                        Ok(true) => {}
                        Ok(false) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json("unsafe_target_field", &format!("Step {idx} target unsafe")));
                        }
                        Err(e) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json(&e.code, &e.message));
                        }
                    }
                    let req = if act == "hover" {
                        ActionRequest::HoverPoint { bundle_id: cur_app.clone(), x, y }
                    } else {
                        let click_count = if act == "double_click" { 2 } else { 1 };
                        let button = if act == "right_click" { "right" } else { "left" };
                        ActionRequest::ClickPoint {
                            bundle_id: cur_app.clone(),
                            x,
                            y,
                            prefer_ax: prefer_ax && click_count == 1 && button == "left",
                            click_count,
                            button: button.into(),
                        }
                    };
                    match self.driver.perform_action(&req) {
                        Ok(res) => step_results.push(JsonValue::object(vec![
                            ("step", JsonValue::Number(idx as f64)),
                            ("action", JsonValue::String(act.into())),
                            ("method", JsonValue::String(res.method)),
                            ("ok", JsonValue::Bool(true)),
                        ])),
                        Err(e) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json(&e.code, &e.message));
                        }
                    }
                }
                "type" => {
                    let Some(text) = step.get("text").and_then(|v| v.as_str()) else {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", &format!("Step {idx} missing text")));
                    };
                    if text.is_empty() || text.chars().count() > 2000 || !text.chars().all(|c| !c.is_control() || matches!(c, '\n' | '\t')) {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", "Invalid batch text"));
                    }
                    let mode = step.get("mode").and_then(|v| v.as_str()).unwrap_or("insert");
                    if !matches!(mode, "insert" | "replace") {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", "mode must be 'insert' or 'replace'"));
                    }
                    let (x, y, _) = match resolve_batch_point(step, &elements_copy, &marks_copy) {
                        Ok(v) => v,
                        Err(e) => {
                            self.invalidate_snapshot();
                            return e;
                        }
                    };
                    let click_first = step.get("click_first").and_then(|v| v.as_bool()).unwrap_or(true);
                    match self.driver.verify_point_safe(&cur_app, x, y) {
                        Ok(true) => {}
                        Ok(false) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json("unsafe_target_field", &format!("Step {idx} target unsafe")));
                        }
                        Err(e) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json(&e.code, &e.message));
                        }
                    }
                    let req = ActionRequest::TypePoint {
                        bundle_id: cur_app.clone(),
                        x,
                        y,
                        text: text.into(),
                        mode: mode.into(),
                        click_first,
                    };
                    match self.driver.perform_action(&req) {
                        Ok(res) => step_results.push(JsonValue::object(vec![
                            ("step", JsonValue::Number(idx as f64)),
                            ("action", JsonValue::String("type".into())),
                            ("method", JsonValue::String(res.method)),
                            ("ok", JsonValue::Bool(true)),
                        ])),
                        Err(e) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json(&e.code, &e.message));
                        }
                    }
                }
                "press_key" => {
                    let Some(key) = step.get("key").and_then(|v| v.as_str()) else {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", &format!("Step {idx} missing key")));
                    };
                    let mods: Vec<String> = step
                        .get("modifiers")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_ascii_lowercase())).collect())
                        .unwrap_or_default();
                    if !valid_key_request(key, &mods) {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", "Unsupported key or blocked paste shortcut"));
                    }
                    let req = ActionRequest::PressKey {
                        bundle_id: cur_app.clone(),
                        key: key.into(),
                        modifiers: mods,
                    };
                    match self.driver.perform_action(&req) {
                        Ok(res) => step_results.push(JsonValue::object(vec![
                            ("step", JsonValue::Number(idx as f64)),
                            ("action", JsonValue::String("press_key".into())),
                            ("method", JsonValue::String(res.method)),
                            ("ok", JsonValue::Bool(true)),
                        ])),
                        Err(e) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json(&e.code, &e.message));
                        }
                    }
                }
                "scroll" => {
                    let Some(dir) = step.get("direction").and_then(|v| v.as_str()) else {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", &format!("Step {idx} missing direction")));
                    };
                    if !matches!(dir, "up" | "down" | "left" | "right") {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", "Invalid scroll direction"));
                    }
                    let amount = step.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
                    if !(1..=100).contains(&amount) {
                        self.invalidate_snapshot();
                        return ToolOutput::json(err_json("invalid_argument", "Scroll amount must be 1..=100"));
                    }
                    let req = ActionRequest::Scroll {
                        bundle_id: cur_app.clone(),
                        direction: dir.into(),
                        amount: amount as i32,
                    };
                    match self.driver.perform_action(&req) {
                        Ok(res) => step_results.push(JsonValue::object(vec![
                            ("step", JsonValue::Number(idx as f64)),
                            ("action", JsonValue::String("scroll".into())),
                            ("method", JsonValue::String(res.method)),
                            ("ok", JsonValue::Bool(true)),
                        ])),
                        Err(e) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json(&e.code, &e.message));
                        }
                    }
                }
                _ => {
                    self.invalidate_snapshot();
                    return ToolOutput::json(err_json("invalid_argument", &format!("Unsupported batch step action: {act}")));
                }
            }
        }

        self.invalidate_snapshot();
        let mut out = vec![
            ("ok", JsonValue::Bool(true)),
            ("policy", JsonValue::String("direct".into())),
            ("executed", JsonValue::Number(step_results.len() as f64)),
            ("results", JsonValue::Array(step_results)),
        ];
        if inspect_after {
            if let Ok(snap) = self.driver.inspect_app(&cur_app, None, 150, 12) {
                if self.check_frontmost_permitted(&snap.frontmost_bundle_id)
                    && snapshot_redaction_reason(&snap).is_none()
                {
                    let outline = snap.render_outline(None);
                    if !contains_secure_marker(&outline) {
                        self.record_snapshot_meta(&snap, None, 150, 12, false);
                        out.push(("content", JsonValue::String(outline)));
                    }
                }
            }
        }
        ToolOutput::json(JsonValue::object(out))
    }

    fn handle_wait_for(&mut self, sub_args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("wait_for", false) {
            return blocked;
        }
        let Some(raw_app) = sub_args.get("app").and_then(|v| v.as_str()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing arguments.app"));
        };
        let Some(target) = self.resolve_target(raw_app) else {
            return ToolOutput::json(err_json("invalid_app", "Target is invalid, protected, or not permitted"));
        };
        let Some(query) = sub_args.get("query").and_then(|v| v.as_str()) else {
            return ToolOutput::json(err_json("invalid_argument", "Missing arguments.query"));
        };
        if query.is_empty() || query.len() > 120 || query.chars().any(|c| c.is_control()) {
            return ToolOutput::json(err_json("invalid_argument", "Invalid arguments.query"));
        }
        let timeout_ms = sub_args
            .get("timeout_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(15_000)
            .clamp(100, 60_000) as u64;
        let poll_ms = sub_args
            .get("poll_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(400)
            .clamp(100, 2_000) as u64;
        let absent = sub_args
            .get("absent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let use_ocr = sub_args
            .get("ocr")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if self.audit.record("wait_for", "allow", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        let start = Instant::now();
        let deadline = Duration::from_millis(timeout_ms);
        let q_lower = query.to_ascii_lowercase();

        loop {
            if self.stop_file.exists() {
                return ToolOutput::json(err_json("panic_stop", "STOP file present"));
            }
            self.driver.tick();
            let snap = match self.driver.inspect_app(&target, None, 150, 12) {
                Ok(s) => s,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            };
            if !self.check_frontmost_permitted(&snap.frontmost_bundle_id) {
                return ToolOutput::json(err_json("invalid_app", "Frontmost application is protected"));
            }
            if let Some(reason) = snapshot_redaction_reason(&snap) {
                return ToolOutput::json(err_json("unsafe_or_incomplete_tree", &reason));
            }
            let full_outline = snap.render_outline(None);
            if contains_secure_marker(&full_outline) {
                return ToolOutput::json(err_json("unsafe_or_incomplete_tree", "sensitive_text_marker"));
            }

            let ax_found = full_outline.to_ascii_lowercase().contains(&q_lower);
            let mut ocr_match_text: Option<String> = None;
            if use_ocr && !ax_found {
                if let Ok(cap) = self.driver.capture_and_ocr(&target, None, OcrMode::Fast, false) {
                    if let Some(bx) = cap
                        .ocr_boxes
                        .iter()
                        .find(|b| b.text.to_ascii_lowercase().contains(&q_lower))
                    {
                        ocr_match_text = Some(bx.text.clone());
                    }
                }
            }

            let found = ax_found || ocr_match_text.is_some();
            if found != absent {
                let elapsed = start.elapsed().as_millis() as f64;
                let source = if ax_found { "ax" } else if ocr_match_text.is_some() { "ocr" } else { "absent" };
                let content = if let Some(txt) = ocr_match_text {
                    txt
                } else {
                    snap.render_outline(if absent { None } else { Some(query) })
                };
                self.invalidate_snapshot();
                self.record_snapshot_meta(&snap, None, 150, 12, !absent && ax_found);
                return ToolOutput::json(JsonValue::object(vec![
                    ("ok", JsonValue::Bool(true)),
                    ("matched", JsonValue::Bool(true)),
                    ("elapsed_ms", JsonValue::Number(elapsed)),
                    ("source", JsonValue::String(source.into())),
                    ("content", JsonValue::String(content)),
                ]));
            }

            let elapsed_now = start.elapsed();
            if elapsed_now >= deadline {
                return ToolOutput::json(JsonValue::object(vec![
                    ("ok", JsonValue::Bool(false)),
                    ("code", JsonValue::String("wait_timeout".into())),
                    ("message", JsonValue::String(format!("Timed out waiting for {query:?}"))),
                    ("elapsed_ms", JsonValue::Number(elapsed_now.as_millis() as f64)),
                ]));
            }
            let rem = deadline - elapsed_now;
            thread::sleep(rem.min(Duration::from_millis(poll_ms)));
        }
    }

    fn handle_memory_replay(&mut self, args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("memory_replay", true) {
            return blocked;
        }
        if self.settings.require_confirmation {
            return ToolOutput::json(err_json(
                "confirmation_required",
                "Workflow replay requires confirmation: direct policy",
            ));
        }
        let all = match self.memory.load_all() {
            Ok(v) => v,
            Err(e) => return ToolOutput::json(err_json("memory_error", &e)),
        };
        let recipe = if let Some(id) = args.get("id").and_then(|v| v.as_str()) {
            all.into_iter().find(|r| r.id == id)
        } else {
            let Some(raw_app) = args.get("app").and_then(|v| v.as_str()) else {
                return ToolOutput::json(err_json("invalid_argument", "Provide arguments.id or arguments.app"));
            };
            let Some(bundle) = self.resolve_target(raw_app) else {
                return ToolOutput::json(err_json("invalid_app", "Invalid or protected app"));
            };
            let goal_q = args.get("goal").and_then(|v| v.as_str()).map(|s| s.to_ascii_lowercase());
            all.into_iter().find(|r| {
                r.app.eq_ignore_ascii_case(&bundle)
                    && goal_q
                        .as_ref()
                        .is_none_or(|g| r.goal.to_ascii_lowercase().contains(g))
            })
        };
        let Some(recipe) = recipe else {
            return ToolOutput::json(err_json("not_found", "Matching workflow recipe not found"));
        };
        let Some(bundle) = self.resolve_target(&recipe.app) else {
            return ToolOutput::json(err_json("invalid_app", "Recipe app is not permitted"));
        };
        if self.audit.record("memory_replay", "direct", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }
        if let Err(e) = self.driver.activate_app(&bundle) {
            return ToolOutput::json(err_json(&e.code, &e.message));
        }
        if self.driver.supports_fast_point_verify() {
            thread::sleep(Duration::from_millis(120));
        }

        let step_args = args.get("step_args").and_then(|v| v.as_array());
        let mut results = Vec::new();
        for (idx, step) in recipe.steps.iter().enumerate() {
            if self.stop_file.exists() {
                self.invalidate_snapshot();
                return ToolOutput::json(err_json("panic_stop", "STOP file present"));
            }
            let override_arg = step_args.and_then(|arr| arr.get(idx));
            let act = step.action.as_str();
            match act {
                "click" | "double_click" | "right_click" | "type" => {
                    let snap = match self.driver.inspect_app(&bundle, None, 250, 14) {
                        Ok(s) => s,
                        Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
                    };
                    if snapshot_redaction_reason(&snap).is_some() {
                        return ToolOutput::json(err_json("unsafe_or_incomplete_tree", "Secure field detected during replay"));
                    }
                    let Some(el) = find_recipe_element(&snap.elements, step) else {
                        self.invalidate_snapshot();
                        return ToolOutput::json(JsonValue::object(vec![
                            ("ok", JsonValue::Bool(false)),
                            ("code", JsonValue::String("replay_target_missing".into())),
                            ("failed_step", JsonValue::Number(idx as f64)),
                            ("target_label", JsonValue::String(step.target_label.clone())),
                            ("completed_steps", JsonValue::Number(results.len() as f64)),
                        ]));
                    };
                    let Some(ref f) = el.frame else {
                        return ToolOutput::json(err_json("target_identity_unverified", "Matched element lacks frame"));
                    };
                    let (cx, cy) = (f.x + f.w * 0.5, f.y + f.h * 0.5);
                    let req = if act == "type" {
                        let Some(text) = override_arg.and_then(|v| v.get("text")).and_then(|v| v.as_str()) else {
                            return ToolOutput::json(err_json("invalid_argument", &format!("Replay step {idx} requires step_args[{idx}].text")));
                        };
                        if text.is_empty() || text.chars().count() > 2000 || !text.chars().all(|c| !c.is_control() || matches!(c, '\n' | '\t')) {
                            return ToolOutput::json(err_json("invalid_argument", "Invalid replay text"));
                        }
                        let is_native_text = matches!(el.role.as_str(), "TextArea" | "TextField" | "SearchField" | "ComboBox");
                        ActionRequest::TypePoint {
                            bundle_id: bundle.clone(),
                            x: cx,
                            y: cy,
                            text: text.into(),
                            mode: "insert".into(),
                            click_first: !is_native_text,
                        }
                    } else {
                        let click_count = if act == "double_click" { 2 } else { 1 };
                        let button = if act == "right_click" { "right" } else { "left" };
                        ActionRequest::ClickPoint {
                            bundle_id: bundle.clone(),
                            x: cx,
                            y: cy,
                            prefer_ax: click_count == 1 && button == "left" && supports_ax_press(el),
                            click_count,
                            button: button.into(),
                        }
                    };
                    match self.driver.perform_action(&req) {
                        Ok(res) => results.push(JsonValue::object(vec![
                            ("step", JsonValue::Number(idx as f64)),
                            ("action", JsonValue::String(act.into())),
                            ("method", JsonValue::String(res.method)),
                        ])),
                        Err(e) => {
                            self.invalidate_snapshot();
                            return ToolOutput::json(err_json(&e.code, &e.message));
                        }
                    }
                }
                "press_key" => {
                    let Some(key) = override_arg.and_then(|v| v.get("key")).and_then(|v| v.as_str()) else {
                        return ToolOutput::json(err_json("invalid_argument", &format!("Replay step {idx} requires step_args[{idx}].key")));
                    };
                    let mods: Vec<String> = override_arg
                        .and_then(|v| v.get("modifiers"))
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_ascii_lowercase())).collect())
                        .unwrap_or_default();
                    if !valid_key_request(key, &mods) {
                        return ToolOutput::json(err_json("invalid_argument", "Unsupported replay key"));
                    }
                    let req = ActionRequest::PressKey {
                        bundle_id: bundle.clone(),
                        key: key.into(),
                        modifiers: mods,
                    };
                    match self.driver.perform_action(&req) {
                        Ok(res) => results.push(JsonValue::object(vec![
                            ("step", JsonValue::Number(idx as f64)),
                            ("action", JsonValue::String("press_key".into())),
                            ("method", JsonValue::String(res.method)),
                        ])),
                        Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
                    }
                }
                _ => {
                    return ToolOutput::json(err_json("invalid_argument", &format!("Unsupported replay action: {act}")));
                }
            }
        }
        self.invalidate_snapshot();
        ToolOutput::json(JsonValue::object(vec![
            ("ok", JsonValue::Bool(true)),
            ("recipe_id", JsonValue::String(recipe.id)),
            ("completed_steps", JsonValue::Number(results.len() as f64)),
            ("results", JsonValue::Array(results)),
        ]))
    }
}

fn resolve_batch_point(
    step: &JsonValue,
    elements: &HashMap<String, UiElement>,
    marks: &HashMap<String, VisualMark>,
) -> Result<(f64, f64, bool), ToolOutput> {
    if let Some(mid) = step.get("mark").and_then(|v| v.as_str()) {
        let Some(m) = marks.get(mid) else {
            return Err(ToolOutput::json(err_json("stale_ref", &format!("Unknown visual mark {mid}"))));
        };
        return Ok((m.center_x, m.center_y, m.prefer_ax));
    }
    if let Some(rid) = step.get("ref").and_then(|v| v.as_str()) {
        let Some(el) = elements.get(rid) else {
            return Err(ToolOutput::json(err_json("stale_ref", &format!("Unknown element ref {rid}"))));
        };
        if el.enabled == Some(false) {
            return Err(ToolOutput::json(err_json("not_implemented", "Element is disabled")));
        }
        let Some(ref f) = el.frame else {
            return Err(ToolOutput::json(err_json("target_identity_unverified", "Element lacks coordinates")));
        };
        return Ok((f.x + f.w * 0.5, f.y + f.h * 0.5, supports_ax_press(el)));
    }
    if let (Some(x), Some(y)) = (
        step.get("x").and_then(|v| v.as_f64()),
        step.get("y").and_then(|v| v.as_f64()),
    ) {
        if !x.is_finite() || !y.is_finite() || x.abs() > 100000.0 || y.abs() > 100000.0 {
            return Err(ToolOutput::json(err_json("invalid_argument", "Coordinate out of bounds")));
        }
        return Ok((x, y, false));
    }
    Err(ToolOutput::json(err_json("invalid_argument", "Step missing mark, ref, or x/y")))
}

fn find_recipe_element<'a>(elements: &'a [UiElement], step: &WorkflowStep) -> Option<&'a UiElement> {
    let want_role = step.target_role.trim();
    let want_label = step.target_label.trim().to_ascii_lowercase();
    let role_matches = |el: &UiElement| {
        el.enabled != Some(false)
            && el.frame.is_some()
            && (want_role.is_empty() || el.role.eq_ignore_ascii_case(want_role))
    };
    if !want_label.is_empty() {
        if let Some(exact) = elements
            .iter()
            .find(|el| role_matches(el) && el.label.trim().to_ascii_lowercase() == want_label)
        {
            return Some(exact);
        }
        if let Some(sub) = elements
            .iter()
            .find(|el| role_matches(el) && el.label.to_ascii_lowercase().contains(&want_label))
        {
            return Some(sub);
        }
        return None;
    }
    elements.iter().find(|el| role_matches(el))
}

fn snapshot_redaction_reason(snap: &AppSnapshot) -> Option<String> {
    if snap.has_secure_field {
        return Some("secure_field_marker".into());
    }
    if snap.has_unverified_field {
        return Some("unverified_field_content".into());
    }
    None
}

fn valid_key_request(key: &str, mods: &[String]) -> bool {
    const NAV: &[&str] = &[
        "return", "enter", "tab", "space", "escape", "esc", "up", "down", "left", "right",
        "home", "end", "pageup", "pagedown", "delete", "backspace", "forwarddelete",
        "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
        "f13", "f14", "f15", "f16", "f17", "f18", "f19", "f20",
    ];
    const ALLOWED_MODS: &[&str] = &["cmd", "ctrl", "alt", "shift"];
    if mods.iter().any(|m| !ALLOWED_MODS.contains(&m.as_str())) {
        return false;
    }
    if key.eq_ignore_ascii_case("v") && mods.iter().any(|m| m == "cmd" || m == "ctrl") {
        return false;
    }
    NAV.contains(&key.to_ascii_lowercase().as_str())
        || (key.len() == 1
            && key.chars().all(|c| c.is_ascii_alphanumeric() || "-=[]';\\,./`".contains(c)))
}

fn err_json(code: &str, message: &str) -> JsonValue {
    JsonValue::object(vec![
        ("ok", JsonValue::Bool(false)),
        ("code", JsonValue::String(code.into())),
        ("message", JsonValue::String(message.into())),
        ("input_emitted", JsonValue::Bool(false)),
    ])
}

fn confirmation_json(token: &str, summary: &str) -> JsonValue {
    JsonValue::object(vec![
        ("ok", JsonValue::Bool(false)),
        ("code", JsonValue::String("confirmation_required".into())),
        ("policy", JsonValue::String("confirm".into())),
        ("input_emitted", JsonValue::Bool(false)),
        ("token", JsonValue::String(token.into())),
        ("summary", JsonValue::String(summary.into())),
    ])
}

/// Pure-Rust minimal RGBA PNG encoder for test fixtures and headless verification.
pub fn encode_solid_png_b64(w: usize, h: usize) -> String {
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let r = ((x * 2) % 255) as u8;
            let g = ((y * 2) % 255) as u8;
            raw.extend_from_slice(&[r, g, 180, 255]);
        }
    }
    let mut zlib = Vec::with_capacity(raw.len() + 64);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut pos = 0;
    while pos < raw.len() {
        let chunk = (raw.len() - pos).min(65535);
        let is_last = if pos + chunk == raw.len() { 1u8 } else { 0u8 };
        zlib.push(is_last);
        zlib.extend_from_slice(&(chunk as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(chunk as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..pos + chunk]);
        pos += chunk;
    }
    let adler = adler32(&raw);
    zlib.extend_from_slice(&adler.to_be_bytes());

    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    push_png_chunk(&mut png, b"IHDR", &ihdr);
    push_png_chunk(&mut png, b"IDAT", &zlib);
    push_png_chunk(&mut png, b"IEND", &[]);
    base64_encode(&png)
}

fn push_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc_in = Vec::with_capacity(4 + data.len());
    crc_in.extend_from_slice(tag);
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

pub fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(((bytes.len() + 2) / 3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Deterministic mock driver used by Rust unit tests and `--mock` MCP smoke tests.
pub struct MockDriver {
    pub click_log: std::sync::Mutex<Vec<String>>,
}

impl MockDriver {
    pub fn new() -> Self {
        Self {
            click_log: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl OsDriver for MockDriver {
    fn doctor(&self) -> Result<DoctorStatus, crate::driver::DriverError> {
        Ok(DoctorStatus {
            os: "mock".into(),
            accessibility: true,
            screen_recording: true,
            note: "Mock driver active for deterministic verification".into(),
        })
    }

    fn list_apps(&self) -> Result<Vec<crate::driver::RunningApp>, crate::driver::DriverError> {
        Ok(vec![
            crate::driver::RunningApp {
                bundle_id: "com.apple.calculator".into(),
                pid: 4242,
                active: true,
            },
            crate::driver::RunningApp {
                bundle_id: "com.apple.TextEdit".into(),
                pid: 4243,
                active: false,
            },
        ])
    }

    fn activate_app(&self, _bundle_id: &str) -> Result<(), crate::driver::DriverError> {
        Ok(())
    }

    fn launch_app(&self, _bundle_id: &str) -> Result<(), crate::driver::DriverError> {
        Ok(())
    }

    fn inspect_app(
        &self,
        bundle_id: &str,
        _window: Option<i32>,
        _max_elements: i32,
        _max_depth: i32,
    ) -> Result<AppSnapshot, crate::driver::DriverError> {
        let front = if bundle_id == "desktop" {
            "com.apple.calculator"
        } else {
            bundle_id
        };
        Ok(AppSnapshot {
            bundle_id: bundle_id.into(),
            frontmost_bundle_id: front.into(),
            pid: 4242,
            frontmost: true,
            windows_total: 1,
            windows: vec![crate::driver::WindowMeta {
                index: 0,
                title: "Calculator".into(),
                frame: Some(Rect {
                    x: 100.0,
                    y: 100.0,
                    w: 320.0,
                    h: 440.0,
                }),
            }],
            elements: vec![
                UiElement {
                    ref_id: "e1".into(),
                    role: "Button".into(),
                    label: "7".into(),
                    value: None,
                    window: 0,
                    depth: 1,
                    interactive: true,
                    enabled: Some(true),
                    focused: false,
                    actions: vec!["AXPress".into()],
                    frame: Some(Rect {
                        x: 120.0,
                        y: 220.0,
                        w: 50.0,
                        h: 40.0,
                    }),
                },
                UiElement {
                    ref_id: "e2".into(),
                    role: "TextArea".into(),
                    label: "Notes".into(),
                    value: Some("".into()),
                    window: 0,
                    depth: 1,
                    interactive: true,
                    enabled: Some(true),
                    focused: true,
                    actions: vec![],
                    frame: Some(Rect {
                        x: 120.0,
                        y: 300.0,
                        w: 220.0,
                        h: 80.0,
                    }),
                },
            ],
            has_secure_field: false,
            has_unverified_field: false,
            truncated: false,
            depth_truncated: false,
            safety_scan_complete: true,
        })
    }

    fn capture_and_ocr(
        &self,
        bundle_id: &str,
        _window: Option<i32>,
        ocr_mode: OcrMode,
        _encode_png: bool,
    ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
        let front = if bundle_id == "desktop" {
            "com.apple.calculator"
        } else {
            bundle_id
        };
        let ocr_boxes = if ocr_mode == OcrMode::Off {
            Vec::new()
        } else {
            vec![crate::driver::OcrBox {
                text: "Canvas Submit".into(),
                confidence: 0.94,
                frame: Rect {
                    x: 210.0,
                    y: 225.0,
                    w: 80.0,
                    h: 28.0,
                },
            }]
        };
        Ok(crate::driver::CaptureResult {
            bundle_id: bundle_id.into(),
            frontmost_bundle_id: front.into(),
            pid: 4242,
            window_id: 99,
            window_frame: Rect {
                x: 100.0,
                y: 100.0,
                w: 320.0,
                h: 440.0,
            },
            image_width: 64,
            image_height: 64,
            png_base64: encode_solid_png_b64(64, 64),
            capture_id: None,
            ocr_boxes,
        })
    }

    fn render_som_png(
        &self,
        png_b64: &str,
        _marks_json: &str,
        _win_frame: &Rect,
    ) -> Result<String, crate::driver::DriverError> {
        Ok(png_b64.to_string())
    }

    fn open_url(&self, bundle_id: &str, url: &str) -> Result<(), crate::driver::DriverError> {
        let mut guard = self.click_log.lock().unwrap();
        guard.push(format!("OpenUrl {{ bundle_id: {bundle_id:?}, url: {url:?} }}"));
        Ok(())
    }

    fn perform_action(
        &self,
        req: &ActionRequest,
    ) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
        let mut guard = self.click_log.lock().unwrap();
        guard.push(format!("{req:?}"));
        Ok(crate::driver::ActionResult {
            method: "mock_ok".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_gateway(tag: &str) -> (Gateway, PathBuf) {
        let dir = std::env::temp_dir().join(format!("handsmcp_gw_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let settings = PolicySettings {
            require_confirmation: true,
            allowed_apps: vec!["*".into()],
            ..PolicySettings::default()
        };
        (Gateway::new(Box::new(MockDriver::new()), settings, dir.clone()), dir)
    }

    #[test]
    fn som_visual_inspect_and_mark_click_confirmation_flow() {
        let (mut gw, dir) = test_gateway("som");
        let vis = gw.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("desktop".into()))]),
        );
        assert!(vis.image_png_b64.is_some());
        assert_eq!(vis.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(vis.payload.get("marks_count").and_then(|v| v.as_i64()), Some(3));

        let click_req = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("visual_click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![("mark", JsonValue::String("m2".into()))]),
                ),
            ]),
        );
        assert_eq!(
            click_req.payload.get("code").and_then(|v| v.as_str()),
            Some("confirmation_required")
        );
        let tok = click_req
            .payload
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        let confirmed = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                (
                    "arguments",
                    JsonValue::object(vec![("token", JsonValue::String(tok.clone()))]),
                ),
            ]),
        );
        assert_eq!(confirmed.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let replay = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                (
                    "arguments",
                    JsonValue::object(vec![("token", JsonValue::String(tok))]),
                ),
            ]),
        );
        assert_eq!(
            replay.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_confirmation")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn drag_right_click_double_click_hover_and_type_by_mark_confirm_cleanly() {
        let (mut gw, dir) = test_gateway("gestures");
        let _ = gw.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );

        for (tool, action, arguments) in [
            (
                "computer",
                "double_click",
                JsonValue::object(vec![("mark", JsonValue::String("m1".into()))]),
            ),
            (
                "computer",
                "right_click",
                JsonValue::object(vec![("ref", JsonValue::String("e1".into()))]),
            ),
            (
                "computer",
                "hover",
                JsonValue::object(vec![("mark", JsonValue::String("m2".into()))]),
            ),
            (
                "computer",
                "drag",
                JsonValue::object(vec![
                    ("from_mark", JsonValue::String("m1".into())),
                    ("to_mark", JsonValue::String("m2".into())),
                ]),
            ),
            (
                "computer",
                "type",
                JsonValue::object(vec![
                    ("mark", JsonValue::String("m2".into())),
                    ("text", JsonValue::String("hello".into())),
                ]),
            ),
        ] {
            // Re-inspect before each confirm since confirmed action invalidates snapshot
            let _ = gw.dispatch(
                "visual_inspect",
                &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
            );
            let req = gw.dispatch(
                tool,
                &JsonValue::object(vec![
                    ("action", JsonValue::String(action.into())),
                    ("arguments", arguments),
                ]),
            );
            assert_eq!(
                req.payload.get("code").and_then(|v| v.as_str()),
                Some("confirmation_required"),
                "expected confirmation for {action}"
            );
            let tok = req.payload.get("token").and_then(|v| v.as_str()).unwrap().to_string();
            let done = gw.dispatch(
                "computer",
                &JsonValue::object(vec![
                    ("action", JsonValue::String("confirm".into())),
                    ("arguments", JsonValue::object(vec![("token", JsonValue::String(tok))])),
                ]),
            );
            assert_eq!(done.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn paste_shortcut_and_protected_apps_are_blocked() {
        let (mut gw, dir) = test_gateway("sec");
        let bad_app = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.1password.app".into()))]),
        );
        assert_eq!(
            bad_app.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_app")
        );

        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        let paste = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("press_key".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("key", JsonValue::String("v".into())),
                        (
                            "modifiers",
                            JsonValue::Array(vec![JsonValue::String("cmd".into())]),
                        ),
                    ]),
                ),
            ]),
        );
        assert_eq!(
            paste.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_argument")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_mode_and_stop_file_and_workflow_memory_work() {
        let dir = std::env::temp_dir().join(format!("handsmcp_gw_ro_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let settings = PolicySettings {
            mode: Mode::ReadOnly,
            allowed_apps: vec!["*".into()],
            ..PolicySettings::default()
        };
        let mut gw = Gateway::new(Box::new(MockDriver::new()), settings, dir.clone());

        let insp = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        assert_eq!(insp.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let clk = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![("ref", JsonValue::String("e1".into()))]),
                ),
            ]),
        );
        assert_eq!(clk.payload.get("code").and_then(|v| v.as_str()), Some("read_only"));

        fs::write(dir.join("STOP"), "halt").unwrap();
        let stopped = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        assert_eq!(
            stopped.payload.get("code").and_then(|v| v.as_str()),
            Some("panic_stop")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    struct TruncatedOutlineDriver {
        safety_complete: bool,
    }

    impl OsDriver for TruncatedOutlineDriver {
        fn doctor(&self) -> Result<DoctorStatus, crate::driver::DriverError> {
            MockDriver::new().doctor()
        }
        fn list_apps(&self) -> Result<Vec<crate::driver::RunningApp>, crate::driver::DriverError> {
            MockDriver::new().list_apps()
        }
        fn activate_app(&self, _: &str) -> Result<(), crate::driver::DriverError> {
            Ok(())
        }
        fn launch_app(&self, _: &str) -> Result<(), crate::driver::DriverError> {
            Ok(())
        }
        fn inspect_app(
            &self,
            bundle_id: &str,
            w: Option<i32>,
            me: i32,
            md: i32,
        ) -> Result<AppSnapshot, crate::driver::DriverError> {
            let mut snap = MockDriver::new().inspect_app(bundle_id, w, me, md)?;
            snap.truncated = true;
            snap.depth_truncated = true;
            snap.safety_scan_complete = self.safety_complete;
            Ok(snap)
        }
        fn capture_and_ocr(
            &self,
            bundle_id: &str,
            w: Option<i32>,
            ocr_mode: OcrMode,
            encode_png: bool,
        ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
            MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_png)
        }
        fn render_som_png(&self, b: &str, m: &str, wf: &Rect) -> Result<String, crate::driver::DriverError> {
            MockDriver::new().render_som_png(b, m, wf)
        }
        fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
            MockDriver::new().perform_action(req)
        }
    }

    #[test]
    fn visual_inspect_allows_truncated_outline_when_safety_scan_complete_and_refuses_incomplete() {
        let dir = std::env::temp_dir().join(format!("handsmcp_gw_trunc_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let settings = PolicySettings {
            require_confirmation: true,
            allowed_apps: vec!["*".into()],
            ..PolicySettings::default()
        };

        let mut gw_ok = Gateway::new(
            Box::new(TruncatedOutlineDriver { safety_complete: true }),
            settings.clone(),
            dir.clone(),
        );
        let ok_res = gw_ok.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );
        assert_eq!(ok_res.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let mut gw_bad = Gateway::new(
            Box::new(TruncatedOutlineDriver { safety_complete: false }),
            settings,
            dir.clone(),
        );
        let bad_res = gw_bad.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );
        assert_eq!(
            bad_res.payload.get("code").and_then(|v| v.as_str()),
            Some("unsafe_or_incomplete_tree")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn direct_confirmation_policy_executes_without_token_and_still_blocks_protected_apps() {
        let dir = std::env::temp_dir().join(format!("handsmcp_gw_direct_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let settings = PolicySettings {
            require_confirmation: false,
            allowed_apps: vec!["*".into()],
            ..PolicySettings::default()
        };
        let mut gw = Gateway::new(Box::new(MockDriver::new()), settings, dir.clone());
        let vis = gw.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        assert_eq!(vis.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let clk = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("visual_click".into())),
                ("arguments", JsonValue::object(vec![("mark", JsonValue::String("m2".into()))])),
            ]),
        );
        assert_eq!(clk.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(clk.payload.get("policy").and_then(|v| v.as_str()), Some("direct"));
        assert_eq!(clk.payload.get("method").and_then(|v| v.as_str()), Some("mock_ok"));

        // Protected apps remain strictly blocked even under direct policy
        let blocked = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.1password.app".into()))]),
        );
        assert_eq!(blocked.payload.get("code").and_then(|v| v.as_str()), Some("invalid_app"));
        let _ = fs::remove_dir_all(&dir);
    }

    struct DynamicTickDriver {
        calls: std::sync::atomic::AtomicUsize,
        remove_target_on_recheck: bool,
    }

    impl OsDriver for DynamicTickDriver {
        fn doctor(&self) -> Result<DoctorStatus, crate::driver::DriverError> {
            MockDriver::new().doctor()
        }
        fn list_apps(&self) -> Result<Vec<crate::driver::RunningApp>, crate::driver::DriverError> {
            MockDriver::new().list_apps()
        }
        fn activate_app(&self, _: &str) -> Result<(), crate::driver::DriverError> {
            Ok(())
        }
        fn launch_app(&self, _: &str) -> Result<(), crate::driver::DriverError> {
            Ok(())
        }
        fn inspect_app(
            &self,
            bundle_id: &str,
            w: Option<i32>,
            me: i32,
            md: i32,
        ) -> Result<AppSnapshot, crate::driver::DriverError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut snap = MockDriver::new().inspect_app(bundle_id, w, me, md)?;
            // Simulate a ticking timer label elsewhere in the app
            if let Some(el) = snap.elements.iter_mut().find(|e| e.role == "StaticText") {
                el.value = Some(format!("tick-{n}"));
            }
            if self.remove_target_on_recheck && n >= 1 {
                snap.elements.retain(|e| e.label != "7");
            }
            Ok(snap)
        }
        fn capture_and_ocr(
            &self,
            bundle_id: &str,
            w: Option<i32>,
            ocr_mode: OcrMode,
            encode_png: bool,
        ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
            MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_png)
        }
        fn render_som_png(&self, b: &str, m: &str, wf: &Rect) -> Result<String, crate::driver::DriverError> {
            MockDriver::new().render_som_png(b, m, wf)
        }
        fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
            MockDriver::new().perform_action(req)
        }
    }

    #[test]
    fn unrelated_static_text_tick_allows_click_while_missing_target_fails_closed() {
        let dir = std::env::temp_dir().join(format!("handsmcp_gw_tick_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let settings = PolicySettings {
            require_confirmation: false,
            allowed_apps: vec!["*".into()],
            ..PolicySettings::default()
        };

        let mut gw_ok = Gateway::new(
            Box::new(DynamicTickDriver {
                calls: std::sync::atomic::AtomicUsize::new(0),
                remove_target_on_recheck: false,
            }),
            settings.clone(),
            dir.clone(),
        );
        gw_ok.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        let clk_ok = gw_ok.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(clk_ok.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let mut gw_stale = Gateway::new(
            Box::new(DynamicTickDriver {
                calls: std::sync::atomic::AtomicUsize::new(0),
                remove_target_on_recheck: true,
            }),
            settings,
            dir.clone(),
        );
        gw_stale.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        let clk_stale = gw_stale.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(clk_stale.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_policy_is_direct_unrestricted_and_allows_web_elements_and_keys() {
        let dir = std::env::temp_dir().join(format!("handsmcp_gw_unres_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let settings = PolicySettings::default();
        assert!(!settings.require_confirmation);
        assert!(settings.permits_app("com.google.Chrome"));

        let mut gw = Gateway::new(Box::new(MockDriver::new()), settings, dir.clone());
        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );

        // Enabled StaticText / non-interactive element e2 can be clicked directly
        let clk = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e2".into()))])),
            ]),
        );
        assert_eq!(clk.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(clk.payload.get("policy").and_then(|v| v.as_str()), Some("direct"));

        // Re-inspect and insert text into StaticText/Group element e2 via click_first fallback
        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );
        let typ = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("type".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("line1\nline2".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(typ.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // Single printable key press works without confirmation
        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );
        let key = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("press_key".into())),
                ("arguments", JsonValue::object(vec![("key", JsonValue::String("/".into()))])),
            ]),
        );
        assert_eq!(key.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn visual_inspect_ocr_off_and_max_marks_filter_output() {
        let (mut gw, dir) = test_gateway("vis_opts");
        let vis_off = gw.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.calculator".into())),
                ("ocr", JsonValue::String("off".into())),
            ]),
        );
        assert_eq!(vis_off.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(vis_off.payload.get("marks_count").and_then(|v| v.as_i64()), Some(2));

        let vis_capped = gw.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.calculator".into())),
                ("ocr", JsonValue::String("fast".into())),
                ("max_marks", JsonValue::Number(1.0)),
            ]),
        );
        assert_eq!(vis_capped.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(vis_capped.payload.get("marks_count").and_then(|v| v.as_i64()), Some(1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn computer_run_actions_executes_chain_and_blocks_under_strict_confirmation() {
        let (mut strict_gw, strict_dir) = test_gateway("batch_strict");
        let _ = strict_gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.calculator".into()))]),
        );
        let denied = strict_gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "steps",
                        JsonValue::Array(vec![JsonValue::object(vec![
                            ("action", JsonValue::String("click".into())),
                            ("ref", JsonValue::String("e1".into())),
                        ])]),
                    )]),
                ),
            ]),
        );
        assert_eq!(
            denied.payload.get("code").and_then(|v| v.as_str()),
            Some("confirmation_required")
        );
        let _ = fs::remove_dir_all(&strict_dir);

        let dir = std::env::temp_dir().join(format!("handsmcp_gw_batch_ok_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gw = Gateway::new(Box::new(MockDriver::new()), PolicySettings::default(), dir.clone());
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.calculator".into()))]),
        );
        let res = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("inspect_after", JsonValue::Bool(true)),
                        (
                            "steps",
                            JsonValue::Array(vec![
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("click".into())),
                                    ("ref", JsonValue::String("e1".into())),
                                ]),
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("type".into())),
                                    ("ref", JsonValue::String("e2".into())),
                                    ("text", JsonValue::String("fast batch".into())),
                                ]),
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("press_key".into())),
                                    ("key", JsonValue::String("enter".into())),
                                ]),
                            ]),
                        ),
                    ]),
                ),
            ]),
        );
        assert_eq!(res.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.payload.get("executed").and_then(|v| v.as_i64()), Some(3));
        assert!(res.payload.get("content").and_then(|v| v.as_str()).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn computer_wait_for_matches_immediately_and_times_out_when_missing() {
        let (mut gw, dir) = test_gateway("waitfor");
        let hit = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("wait_for".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.apple.calculator".into())),
                        ("query", JsonValue::String("Notes".into())),
                        ("timeout_ms", JsonValue::Number(500.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(hit.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(hit.payload.get("source").and_then(|v| v.as_str()), Some("ax"));

        let miss = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("wait_for".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.apple.calculator".into())),
                        ("query", JsonValue::String("NeverAppears".into())),
                        ("timeout_ms", JsonValue::Number(120.0)),
                        ("poll_ms", JsonValue::Number(100.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(miss.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(miss.payload.get("code").and_then(|v| v.as_str()), Some("wait_timeout"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn desktop_open_url_validates_urls_and_executes() {
        let dir = std::env::temp_dir().join(format!("handsmcp_gw_openurl_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gw = Gateway::new(Box::new(MockDriver::new()), PolicySettings::default(), dir.clone());

        let ok = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("open_url".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.google.Chrome".into())),
                        ("url", JsonValue::String("https://example.com/upload".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(ok.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let bad_scheme = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("open_url".into())),
                (
                    "arguments",
                    JsonValue::object(vec![("url", JsonValue::String("javascript:alert(1)".into()))]),
                ),
            ]),
        );
        assert_eq!(bad_scheme.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        let protected = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("open_url".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.1password.1password".into())),
                        ("url", JsonValue::String("https://example.com".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(protected.payload.get("code").and_then(|v| v.as_str()), Some("invalid_app"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn workflow_memory_replay_executes_saved_steps_with_args() {
        let dir = std::env::temp_dir().join(format!("handsmcp_gw_replay_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gw = Gateway::new(Box::new(MockDriver::new()), PolicySettings::default(), dir.clone());

        let rec = gw.dispatch(
            "workflow_memory",
            &JsonValue::object(vec![
                ("action", JsonValue::String("record".into())),
                ("app", JsonValue::String("com.apple.calculator".into())),
                ("goal", JsonValue::String("tap seven and write note".into())),
                (
                    "steps",
                    JsonValue::Array(vec![
                        JsonValue::object(vec![
                            ("action", JsonValue::String("click".into())),
                            ("target_role", JsonValue::String("Button".into())),
                            ("target_label", JsonValue::String("7".into())),
                        ]),
                        JsonValue::object(vec![
                            ("action", JsonValue::String("type".into())),
                            ("target_role", JsonValue::String("TextArea".into())),
                            ("target_label", JsonValue::String("Notes".into())),
                        ]),
                    ]),
                ),
            ]),
        );
        assert_eq!(rec.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let rep = gw.dispatch(
            "workflow_memory",
            &JsonValue::object(vec![
                ("action", JsonValue::String("replay".into())),
                ("app", JsonValue::String("com.apple.calculator".into())),
                ("goal", JsonValue::String("tap seven".into())),
                (
                    "step_args",
                    JsonValue::Array(vec![
                        JsonValue::Null,
                        JsonValue::object(vec![("text", JsonValue::String("hello replay".into()))]),
                    ]),
                ),
            ]),
        );
        assert_eq!(rep.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(rep.payload.get("completed_steps").and_then(|v| v.as_i64()), Some(2));
        let _ = fs::remove_dir_all(&dir);
    }
}
