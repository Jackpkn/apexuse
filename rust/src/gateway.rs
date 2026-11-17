use crate::config::{AppConfig, ImageFormat, TimingConfig};
use crate::driver::common::normalize_role;
use crate::driver::{
    is_editable_role, reconcile_and_diff_snapshot, ActionRequest, AppSnapshot, DoctorStatus,
    ImageEncodeOptions, OcrMode, OnboardStatus, OsDriver, Rect, UiElement, WindowState,
};
use crate::json::JsonValue;
use crate::memory::{WorkflowMemory, WorkflowStep};
use crate::policy::{
    contains_secure_marker, domain_matches, extract_url_host, is_destructive_label, valid_app_id,
    AuditLog, ConfirmationStore, Mode, PendingAction, PolicySettings, RateLimiter,
};
use crate::som::{format_mark_index, fuse_marks, supports_ax_press, VisualMark};
use crate::som_png::{
    base64_decode, decode_png_rgba, downscale_rgba, encode_rgba_image_b64, encode_rgba_png_b64,
    render_som_image_to_b64,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    Auto,
    Ax,
    Visual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnStateMode {
    None,
    Diff,
    Full,
}

enum SnapshotRedaction {
    ProtectedFrontmost,
    DomainBlocked(String),
    Reason(String),
}

impl SnapshotRedaction {
    fn reason_str(&self) -> &str {
        match self {
            SnapshotRedaction::ProtectedFrontmost => "protected_frontmost_app",
            SnapshotRedaction::DomainBlocked(_) => "domain_blocked",
            SnapshotRedaction::Reason(r) => r.as_str(),
        }
    }
}

#[derive(Debug, Clone)]
struct ActionVerifyOptions {
    verify_change: bool,
    verify_mode: VerifyMode,
    return_state: ReturnStateMode,
    include_bounds: Option<bool>,
    max_retries: u8,
    settle_ms: u64,
    poll_interval_ms: u64,
    retry_delay_ms: u64,
    require_change: bool,
    keep_snapshot: Option<bool>,
}

impl ActionVerifyOptions {
    fn from_timing(timing: &TimingConfig) -> Self {
        Self {
            verify_change: false,
            verify_mode: VerifyMode::Auto,
            return_state: ReturnStateMode::None,
            include_bounds: None,
            max_retries: 0,
            settle_ms: timing.settle_ms,
            poll_interval_ms: timing.poll_interval_ms,
            retry_delay_ms: 250,
            require_change: false,
            keep_snapshot: None,
        }
    }
}

impl Default for ActionVerifyOptions {
    fn default() -> Self {
        Self::from_timing(&TimingConfig::default())
    }
}

#[derive(Debug, Clone, Copy)]
struct VerifyTelemetry {
    verified_change: bool,
    ax_changed: bool,
    visual_changed: bool,
    attempts: u8,
    retried: u8,
}

#[derive(Debug, Clone)]
enum GatewayAction {
    Std(PendingAction),
    SetValue {
        epoch: u64,
        ref_id: Option<String>,
        x: f64,
        y: f64,
        text: String,
    },
    SelectText {
        epoch: u64,
        ref_id: Option<String>,
        x: f64,
        y: f64,
        start: Option<usize>,
        end: Option<usize>,
    },
}

#[derive(Debug, Clone)]
struct SemanticTargetSpec {
    query: String,
    role: Option<String>,
    fresh: bool,
}

enum SemanticMatchError {
    NotFound,
    Ambiguous,
    Disabled,
}

pub struct ToolOutput {
    pub payload: JsonValue,
    pub image_b64: Option<String>,
    pub image_mime_type: &'static str,
    pub image_caption: Option<String>,
}

impl ToolOutput {
    pub fn json(payload: JsonValue) -> Self {
        Self {
            payload,
            image_b64: None,
            image_mime_type: ImageFormat::Png.mime_type(),
            image_caption: None,
        }
    }
}

pub struct Gateway {
    driver: Box<dyn OsDriver>,
    settings: PolicySettings,
    config: AppConfig,
    audit: AuditLog,
    memory: WorkflowMemory,
    stop_file: PathBuf,
    rate_limiter: RateLimiter,
    confirmations: ConfirmationStore,
    pending_verify: HashMap<String, ActionVerifyOptions>,
    pending_gateway_actions: HashMap<String, GatewayAction>,
    epoch: u64,
    snapshot_app: Option<String>,
    snapshot_window: Option<i32>,
    snapshot_max_elements: i32,
    snapshot_max_depth: i32,
    snapshot_filtered: bool,
    snapshot_pid: Option<i64>,
    snapshot_frontmost: Option<String>,
    snapshot_windows_total: Option<usize>,
    snapshot_active_frame: Option<Rect>,
    snapshot_ws_pid: Option<i64>,
    snapshot_ws_windows_total: Option<usize>,
    snapshot_sheets: usize,
    snapshot_comparable: Option<String>,
    prev_ax_snapshot: Option<AppSnapshot>,
    prev_ax_window: Option<i32>,
    ax_next_ref: usize,
    elements_by_ref: HashMap<String, UiElement>,
    marks_by_id: HashMap<String, VisualMark>,
}

impl Gateway {
    pub fn new(
        driver: Box<dyn OsDriver>,
        settings: PolicySettings,
        state_dir: PathBuf,
    ) -> Self {
        Self::with_config(driver, settings, state_dir, AppConfig::default())
    }

    pub fn with_config(
        driver: Box<dyn OsDriver>,
        settings: PolicySettings,
        state_dir: PathBuf,
        config: AppConfig,
    ) -> Self {
        let audit = AuditLog::new(state_dir.join("logs/audit.jsonl"));
        let memory = WorkflowMemory::new(state_dir.join("memory/recipes.json"));
        let stop_file = state_dir.join("STOP");
        let rate_limiter = RateLimiter::new(settings.rate_limit_per_minute);
        let confirmations = ConfirmationStore::new(settings.confirmation_ttl_seconds);
        Self {
            driver,
            settings,
            config,
            audit,
            memory,
            stop_file,
            rate_limiter,
            confirmations,
            pending_verify: HashMap::new(),
            pending_gateway_actions: HashMap::new(),
            epoch: 0,
            snapshot_app: None,
            snapshot_window: None,
            snapshot_max_elements: 150,
            snapshot_max_depth: 12,
            snapshot_filtered: false,
            snapshot_pid: None,
            snapshot_frontmost: None,
            snapshot_windows_total: None,
            snapshot_active_frame: None,
            snapshot_ws_pid: None,
            snapshot_ws_windows_total: None,
            snapshot_sheets: 0,
            snapshot_comparable: None,
            prev_ax_snapshot: None,
            prev_ax_window: None,
            ax_next_ref: 1,
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
            }) => {
                let mut fields = vec![
                    ("ok", JsonValue::Bool(true)),
                    ("os", JsonValue::String(os.clone())),
                    ("accessibility", JsonValue::Bool(accessibility)),
                    ("screen_recording", JsonValue::Bool(screen_recording)),
                    ("note", JsonValue::String(note)),
                    (
                        "mode",
                        JsonValue::String(self.settings.mode.as_str().to_string()),
                    ),
                ];
                if os == "macos" && (!accessibility || !screen_recording) {
                    fields.push((
                        "onboard_hint",
                        JsonValue::String(
                            "Run 'apexuse-mcp doctor --onboard' to open guided setup".into(),
                        ),
                    ));
                }
                JsonValue::object(fields)
            }
            Err(e) => err_json(&e.code, &e.message),
        }
    }

    pub fn onboard(&self, force: bool, auto_close: bool) -> JsonValue {
        match self.driver.onboard(force, auto_close) {
            Ok(OnboardStatus {
                os,
                shown,
                already_granted,
                accessibility,
                screen_recording,
                completed,
                host_app,
                note,
            }) => {
                let mut fields = vec![
                    ("ok", JsonValue::Bool(true)),
                    ("os", JsonValue::String(os)),
                    ("shown", JsonValue::Bool(shown)),
                    ("already_granted", JsonValue::Bool(already_granted)),
                    ("accessibility", JsonValue::Bool(accessibility)),
                    ("screen_recording", JsonValue::Bool(screen_recording)),
                    ("completed", JsonValue::Bool(completed)),
                    (
                        "host_app",
                        match host_app {
                            Some(h) => JsonValue::String(h),
                            None => JsonValue::Null,
                        },
                    ),
                ];
                if !note.is_empty() {
                    fields.push(("note", JsonValue::String(note)));
                }
                JsonValue::object(fields)
            }
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
        self.snapshot_filtered = false;
        self.snapshot_pid = None;
        self.snapshot_frontmost = None;
        self.snapshot_windows_total = None;
        self.snapshot_active_frame = None;
        self.snapshot_ws_pid = None;
        self.snapshot_ws_windows_total = None;
        self.snapshot_sheets = 0;
        self.snapshot_comparable = None;
        self.elements_by_ref.clear();
        self.marks_by_id.clear();
        self.confirmations.clear();
        self.pending_verify.clear();
        self.pending_gateway_actions.clear();
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
        if let Ok(ws) = self.driver.window_state(&snap.bundle_id) {
            self.snapshot_frontmost = Some(ws.frontmost_bundle_id);
            self.snapshot_active_frame = ws.active_window_frame;
            self.snapshot_ws_pid = Some(ws.pid);
            self.snapshot_ws_windows_total = Some(ws.windows_total);
        } else {
            self.snapshot_frontmost = None;
            self.snapshot_active_frame = None;
            self.snapshot_ws_pid = None;
            self.snapshot_ws_windows_total = None;
        }
        self.snapshot_sheets = snap.elements.iter().filter(|e| e.role == "Sheet").count();
        self.snapshot_comparable = Some(snap.comparable_text());
        for el in &snap.elements {
            if !el.ref_id.is_empty() {
                self.elements_by_ref.insert(el.ref_id.clone(), el.clone());
            }
        }
    }

    fn should_preserve_snapshot(&self, app: &str) -> bool {
        let (Some(front), Some(ws_pid), Some(ws_total)) = (
            self.snapshot_frontmost.as_deref(),
            self.snapshot_ws_pid,
            self.snapshot_ws_windows_total,
        ) else {
            return false;
        };
        let Ok(current) = self.driver.window_state(app) else {
            return false;
        };
        front == current.frontmost_bundle_id
            && ws_pid == current.pid
            && ws_total == current.windows_total
            && rects_approx_equal(
                self.snapshot_active_frame.as_ref(),
                current.active_window_frame.as_ref(),
            )
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

    fn check_url_domain_allowed(&self, url: &str, action: &str) -> Result<(), ToolOutput> {
        if !self.settings.has_domain_restrictions() {
            return Ok(());
        }
        match extract_url_host(url) {
            Some(host) if self.settings.permits_domain(&host) => Ok(()),
            Some(host) => {
                let _ = self.audit.record(action, "deny", "domain_blocked");
                Err(ToolOutput::json(err_json(
                    "domain_blocked",
                    &format!("Domain '{host}' is blocked by policy"),
                )))
            }
            None => {
                let _ = self.audit.record(action, "deny", "domain_blocked");
                Err(ToolOutput::json(err_json(
                    "domain_blocked",
                    "Target URL host is blocked or unverified by domain policy",
                )))
            }
        }
    }

    fn check_snapshot_domain_allowed(&self, snap: &AppSnapshot) -> Result<(), String> {
        if !self.settings.has_domain_restrictions() {
            return Ok(());
        }
        if let Some(url) = snapshot_page_url(snap) {
            if let Some(host) = extract_url_host(&url) {
                if !self.settings.permits_domain(&host) {
                    return Err(format!("Domain '{host}' is blocked by policy"));
                }
            }
        }
        Ok(())
    }

    fn check_ax_snapshot_safety(&mut self, snap: &AppSnapshot) -> Result<(), SnapshotRedaction> {
        if !self.check_frontmost_permitted(&snap.frontmost_bundle_id) {
            self.prev_ax_snapshot = None;
            self.prev_ax_window = None;
            return Err(SnapshotRedaction::ProtectedFrontmost);
        }
        if let Err(msg) = self.check_snapshot_domain_allowed(snap) {
            self.prev_ax_snapshot = None;
            self.prev_ax_window = None;
            return Err(SnapshotRedaction::DomainBlocked(msg));
        }
        if let Some(reason) = snapshot_redaction_reason(snap) {
            self.prev_ax_snapshot = None;
            self.prev_ax_window = None;
            return Err(SnapshotRedaction::Reason(reason));
        }
        // Always check full unfiltered outline so diffing or query filtering never hides markers.
        if contains_secure_marker(&snap.render_outline(None)) {
            self.prev_ax_snapshot = None;
            self.prev_ax_window = None;
            return Err(SnapshotRedaction::Reason("sensitive_text_marker".into()));
        }
        Ok(())
    }

    fn finalize_ax_outline_and_baseline(
        &mut self,
        mut snap: AppSnapshot,
        window: Option<i32>,
        query: Option<&str>,
        diff_enabled: bool,
        include_bounds: bool,
    ) -> (String, &'static str, AppSnapshot) {
        if let Some(q) = query {
            let content = snap.render_outline_with_options(Some(q), include_bounds);
            return (content, "filtered", snap);
        }
        let desktop_frontmost_changed = self.prev_ax_snapshot.as_ref().is_some_and(|p| {
            (p.bundle_id == "desktop" || snap.bundle_id == "desktop")
                && p.frontmost_bundle_id != snap.frontmost_bundle_id
        });
        let force_full =
            !diff_enabled || self.prev_ax_window != window || desktop_frontmost_changed;
        let prev = if force_full {
            None
        } else {
            self.prev_ax_snapshot.as_ref()
        };
        let result = reconcile_and_diff_snapshot(
            prev,
            &mut snap,
            &mut self.ax_next_ref,
            include_bounds,
            force_full,
        );
        self.prev_ax_snapshot = Some(snap.clone());
        self.prev_ax_window = window;
        (result.content, result.mode.as_str(), snap)
    }

    fn attach_post_action_state(
        &mut self,
        app: &str,
        window: Option<i32>,
        return_state: ReturnStateMode,
        include_bounds: bool,
        fields: &mut Vec<(&'static str, JsonValue)>,
    ) -> bool {
        if return_state == ReturnStateMode::None {
            return false;
        }
        let snap = match self.driver.inspect_app(app, window, 150, 12) {
            Ok(s) => s,
            Err(e) => {
                self.invalidate_snapshot();
                self.prev_ax_snapshot = None;
                self.prev_ax_window = None;
                fields.push(("snapshot_active", JsonValue::Bool(false)));
                fields.push(("state_error", JsonValue::String(e.code)));
                return false;
            }
        };
        if let Err(redaction) = self.check_ax_snapshot_safety(&snap) {
            let _ = self.audit.record("inspect", "deny", redaction.reason_str());
            self.invalidate_snapshot();
            fields.push(("snapshot_active", JsonValue::Bool(false)));
            let msg = if matches!(redaction, SnapshotRedaction::DomainBlocked(_)) {
                "[blocked by domain policy]"
            } else {
                "[redacted: secure field detected]"
            };
            fields.push(("content", JsonValue::String(msg.into())));
            fields.push((
                "redaction_reason",
                JsonValue::String(redaction.reason_str().into()),
            ));
            return false;
        }
        let raw_comparable = snap.comparable_text();
        let diff_enabled = return_state == ReturnStateMode::Diff;
        let (content, state_mode, reconciled_snap) = self.finalize_ax_outline_and_baseline(
            snap,
            window,
            None,
            diff_enabled,
            include_bounds,
        );
        self.invalidate_snapshot();
        self.record_snapshot_meta(&reconciled_snap, window, 150, 12, false);
        self.snapshot_comparable = Some(raw_comparable);
        fields.push(("content", JsonValue::String(content)));
        fields.push(("state_mode", JsonValue::String(state_mode.into())));
        fields.push(("snapshot_active", JsonValue::Bool(true)));
        true
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
        let window = match parse_optional_window(args) {
            Ok(w) => w,
            Err(e) => return e,
        };
        let query = match parse_optional_query(args) {
            Ok(q) => q,
            Err(e) => return e,
        };
        let diff_enabled = match parse_optional_bool(args, "diff", self.config.snapshot.ax_diff) {
            Ok(b) => b,
            Err(e) => return e,
        };
        let include_bounds = match parse_optional_bool(args, "include_bounds", self.config.snapshot.include_bounds) {
            Ok(b) => b,
            Err(e) => return e,
        };

        if self.audit.record("inspect", "allow", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        self.invalidate_snapshot();
        let snap = match self.driver.inspect_app(&target, window, 150, 12) {
            Ok(s) => s,
            Err(e) => {
                self.prev_ax_snapshot = None;
                self.prev_ax_window = None;
                return ToolOutput::json(err_json(&e.code, &e.message));
            }
        };

        if let Err(redaction) = self.check_ax_snapshot_safety(&snap) {
            return match redaction {
                SnapshotRedaction::ProtectedFrontmost => {
                    let _ = self.audit.record("inspect", "deny", "blocked");
                    ToolOutput::json(err_json("invalid_app", "Frontmost application is protected"))
                }
                SnapshotRedaction::DomainBlocked(msg) => {
                    let _ = self.audit.record("inspect", "deny", "domain_blocked");
                    ToolOutput::json(err_json("domain_blocked", &msg))
                }
                SnapshotRedaction::Reason(reason) => {
                    let _ = self.audit.record("inspect", "deny", "secure_field");
                    ToolOutput::json(JsonValue::object(vec![
                        ("ok", JsonValue::Bool(true)),
                        ("content", JsonValue::String("[redacted: secure field detected]".into())),
                        ("redaction_reason", JsonValue::String(reason)),
                    ]))
                }
            };
        }

        let raw_comparable = snap.comparable_text();
        let (content, state_mode, reconciled_snap) = self.finalize_ax_outline_and_baseline(
            snap,
            window,
            query,
            diff_enabled,
            include_bounds,
        );

        self.record_snapshot_meta(&reconciled_snap, window, 150, 12, query.is_some());
        self.snapshot_comparable = Some(raw_comparable);

        let hint_bundle = if reconciled_snap.bundle_id == "desktop" {
            &reconciled_snap.frontmost_bundle_id
        } else {
            &reconciled_snap.bundle_id
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
            ("app", JsonValue::String(reconciled_snap.bundle_id)),
            ("frontmost_app", JsonValue::String(reconciled_snap.frontmost_bundle_id)),
            ("pid", JsonValue::Number(reconciled_snap.pid as f64)),
            ("windows_total", JsonValue::Number(reconciled_snap.windows_total as f64)),
            ("state_mode", JsonValue::String(state_mode.into())),
            ("content", JsonValue::String(content)),
            ("workflow_hints", JsonValue::Array(hints)),
        ]))
    }

    fn validate_visual_snapshot(&self, target: &str, full: &AppSnapshot) -> Result<(), ToolOutput> {
        if !self.check_frontmost_permitted(&full.frontmost_bundle_id) {
            return Err(ToolOutput::json(err_json("invalid_app", "Frontmost application is protected")));
        }
        if let Err(msg) = self.check_snapshot_domain_allowed(full) {
            let _ = self.audit.record("screenshot", "deny", "domain_blocked");
            return Err(ToolOutput::json(err_json("domain_blocked", &msg)));
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
        let window = match parse_optional_window(args) {
            Ok(w) => w,
            Err(e) => return e,
        };
        let ocr_mode = match args.get("ocr") {
            None | Some(JsonValue::Null) => OcrMode::Fast,
            Some(v) => match v.as_str() {
                Some("auto") | Some("fast") => OcrMode::Fast,
                Some("accurate") => OcrMode::Accurate,
                Some("off") | Some("none") => OcrMode::Off,
                _ => {
                    return ToolOutput::json(err_json(
                        "invalid_argument",
                        "'ocr' must be auto, fast, accurate, or off",
                    ))
                }
            },
        };
        let max_marks = match args.get("max_marks") {
            None | Some(JsonValue::Null) => 80usize,
            Some(v) => match v.as_i64() {
                Some(n) if (1..=150).contains(&n) => n as usize,
                _ => return ToolOutput::json(err_json("invalid_argument", "'max_marks' must be 1..=150")),
            },
        };
        let img_opts = match resolve_image_options(args, &self.config) {
            Ok(o) => o,
            Err(e) => return e,
        };
        if self.audit.record("screenshot", "allow", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        self.invalidate_snapshot();
        let (full, cap) = if target != "desktop" && window.is_none() {
            // Run AX safety walk and ScreenCaptureKit + Vision OCR concurrently.
            let driver = &*self.driver;
            let (ax_res, cap_res) = thread::scope(|s| {
                let cap_handle = s.spawn(|| driver.capture_and_ocr(&target, None, ocr_mode, false, &img_opts));
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
            let cap = match self.driver.capture_and_ocr(&full.bundle_id, window, ocr_mode, false, &img_opts) {
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

        let raw_b64 = if marks.is_empty()
            && !cap.image_base64.is_empty()
            && cap.image_format == img_opts.format
            && (cap.image_width == 0 || cap.image_width <= img_opts.max_width)
        {
            cap.image_base64.clone()
        } else {
            let fallback_src = if !cap.png_base64.is_empty() {
                cap.png_base64.as_str()
            } else {
                cap.image_base64.as_str()
            };
            let img_ref = cap.capture_id.as_deref().unwrap_or(fallback_src);
            match self.driver.render_som_image(
                img_ref,
                &marks_overlay.to_json_string(),
                &cap.window_frame,
                &img_opts,
            ) {
                Ok(b) => b,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            }
        };
        let (annotated_b64, out_w, out_h) = match ensure_encoded_image_b64(
            raw_b64,
            cap.image_width,
            cap.image_height,
            &img_opts,
        ) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let image_bytes = base64_decoded_len(&annotated_b64);

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
                ("image_format", JsonValue::String(img_opts.format.as_str().into())),
                ("image_width", JsonValue::Number(out_w as f64)),
                ("image_height", JsonValue::Number(out_h as f64)),
                ("image_bytes", JsonValue::Number(image_bytes as f64)),
            ]),
            image_b64: Some(annotated_b64),
            image_mime_type: img_opts.format.mime_type(),
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
        let img_opts = match resolve_image_options(args, &self.config) {
            Ok(o) => o,
            Err(e) => return e,
        };
        if self.audit.record("screenshot", "allow", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        self.invalidate_snapshot();
        let full = match self.verify_visual_safety(&target) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let cap = match self.driver.capture_and_ocr(
            &full.bundle_id,
            None,
            OcrMode::Off,
            true,
            &img_opts,
        ) {
            Ok(c) => c,
            Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
        };

        let raw_b64 = if cap.image_format == img_opts.format && !cap.image_base64.is_empty() {
            cap.image_base64.clone()
        } else if !cap.png_base64.is_empty() {
            cap.png_base64.clone()
        } else if let Some(ref cid) = cap.capture_id {
            match self.driver.render_som_image(cid, "[]", &cap.window_frame, &img_opts) {
                Ok(b) => b,
                Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
            }
        } else {
            cap.image_base64.clone()
        };
        let (img_b64, out_w, out_h) = match ensure_encoded_image_b64(
            raw_b64,
            cap.image_width,
            cap.image_height,
            &img_opts,
        ) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let image_bytes = base64_decoded_len(&img_b64);

        let caption = format!(
            "Captured {} ({}x{}).",
            full.bundle_id, out_w, out_h
        );
        ToolOutput {
            payload: JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                ("app", JsonValue::String(full.bundle_id)),
                ("window_id", JsonValue::Number(cap.window_id as f64)),
                ("image_format", JsonValue::String(img_opts.format.as_str().into())),
                ("image_width", JsonValue::Number(out_w as f64)),
                ("image_height", JsonValue::Number(out_h as f64)),
                ("image_bytes", JsonValue::Number(image_bytes as f64)),
            ]),
            image_b64: Some(img_b64),
            image_mime_type: img_opts.format.mime_type(),
            image_caption: Some(caption),
        }
    }

    fn resolve_point(&self, args: &JsonValue, ref_key: &str, mark_key: &str, x_key: &str, y_key: &str) -> Result<(f64, f64, bool, String), ToolOutput> {
        resolve_point_from_maps(
            args,
            &self.elements_by_ref,
            &self.marks_by_id,
            ref_key,
            mark_key,
            x_key,
            y_key,
            false,
        )
    }

    fn has_active_unfiltered_snapshot(&self, target_app: &str, req_window: Option<i32>) -> bool {
        self.snapshot_pid.is_some()
            && !self.snapshot_filtered
            && self.snapshot_app.as_deref() == Some(target_app)
            && (req_window.is_none() || self.snapshot_window == req_window)
    }

    fn resolve_semantic_target(
        &mut self,
        default_app: Option<&str>,
        args: &JsonValue,
        spec: &SemanticTargetSpec,
    ) -> Result<(String, UiElement), ToolOutput> {
        let target_app = match args.get("app") {
            None | Some(JsonValue::Null) => match default_app {
                Some(a) => a.to_string(),
                None => {
                    return Err(ToolOutput::json(err_json(
                        "stale_ref",
                        "Inspect an app or pass arguments.app first",
                    )))
                }
            },
            Some(v) => {
                let raw = v.as_str().ok_or_else(|| {
                    ToolOutput::json(err_json("invalid_argument", "'app' must be a string"))
                })?;
                self.resolve_target(raw).ok_or_else(|| {
                    ToolOutput::json(err_json(
                        "invalid_app",
                        "Target is invalid, protected, or not permitted",
                    ))
                })?
            }
        };

        let window = parse_optional_window(args)?;

        if !spec.fresh && self.has_active_unfiltered_snapshot(&target_app, window) {
            match match_semantic_in_elements(&self.elements_by_ref, spec) {
                Ok(el) => return Ok((target_app, el)),
                Err(SemanticMatchError::Ambiguous) => {
                    return Err(ToolOutput::json(err_json(
                        "ambiguous_target",
                        &format!("Multiple elements matched target query {:?}", spec.query),
                    )))
                }
                Err(SemanticMatchError::Disabled) => {
                    return Err(ToolOutput::json(err_json(
                        "not_implemented",
                        "Matched target element is disabled",
                    )))
                }
                Err(SemanticMatchError::NotFound) => {}
            }
        }

        if self.stop_file.exists() {
            return Err(ToolOutput::json(err_json("panic_stop", "STOP file present")));
        }
        self.invalidate_snapshot();
        let snap = self
            .driver
            .inspect_app(&target_app, window, 150, 12)
            .map_err(|e| ToolOutput::json(err_json(&e.code, &e.message)))?;
        if let Err(redaction) = self.check_ax_snapshot_safety(&snap) {
            return Err(redaction_to_error(redaction));
        }
        if !snap.safety_scan_complete {
            return Err(ToolOutput::json(err_json(
                "unsafe_or_incomplete_tree",
                "Safety scan incomplete or secure field detected",
            )));
        }

        self.record_snapshot_meta(&snap, window, 150, 12, false);

        match match_semantic_in_elements(&self.elements_by_ref, spec) {
            Ok(el) => Ok((snap.bundle_id, el)),
            Err(SemanticMatchError::NotFound) => Err(ToolOutput::json(err_json(
                "target_not_found",
                &format!("No element matched target query {:?}", spec.query),
            ))),
            Err(SemanticMatchError::Ambiguous) => Err(ToolOutput::json(err_json(
                "ambiguous_target",
                &format!("Multiple elements matched target query {:?}", spec.query),
            ))),
            Err(SemanticMatchError::Disabled) => Err(ToolOutput::json(err_json(
                "not_implemented",
                "Matched target element is disabled",
            ))),
        }
    }

    fn resolve_editable_target(
        &mut self,
        action_name: &str,
        args: &JsonValue,
        default_app: Option<&str>,
    ) -> Result<(String, Option<String>, f64, f64, String), ToolOutput> {
        if let Some(spec) = parse_semantic_target(args)? {
            let (app, el) = self.resolve_semantic_target(default_app, args, &spec)?;
            if !is_editable_role(&el.role) || contains_secure_marker(&el.label) {
                return Err(ToolOutput::json(err_json(
                    "target_identity_unverified",
                    &format!("{action_name} requires a verified editable AX control"),
                )));
            }
            let Some(ref f) = el.frame else {
                return Err(ToolOutput::json(err_json(
                    "target_identity_unverified",
                    "Target lacks bounding frame",
                )));
            };
            return Ok((
                app,
                Some(el.ref_id.clone()),
                f.x + f.w * 0.5,
                f.y + f.h * 0.5,
                format!("{} '{}' ({})", el.role, el.label, el.ref_id),
            ));
        }

        if has_non_null(args, "x") || has_non_null(args, "y") {
            return Err(ToolOutput::json(err_json(
                "target_identity_unverified",
                &format!("{action_name} requires a verified editable AX ref, editable mark, or target; raw x/y coordinates are rejected"),
            )));
        }

        let Some(app) = default_app.map(str::to_string) else {
            return Err(ToolOutput::json(err_json("stale_ref", "Inspect an app first")));
        };

        if let Some(rid) = args.get("ref").and_then(|v| v.as_str()) {
            let Some(el) = self.elements_by_ref.get(rid).cloned() else {
                return Err(ToolOutput::json(err_json(
                    "stale_ref",
                    "Unknown or stale element ref",
                )));
            };
            if el.enabled == Some(false) {
                return Err(ToolOutput::json(err_json(
                    "not_implemented",
                    "Element is disabled",
                )));
            }
            if !is_editable_role(&el.role) || contains_secure_marker(&el.label) {
                return Err(ToolOutput::json(err_json(
                    "target_identity_unverified",
                    &format!("{action_name} target role '{}' is not editable", el.role),
                )));
            }
            let Some(ref f) = el.frame else {
                return Err(ToolOutput::json(err_json(
                    "target_identity_unverified",
                    "Target lacks bounding frame",
                )));
            };
            return Ok((
                app,
                Some(rid.to_string()),
                f.x + f.w * 0.5,
                f.y + f.h * 0.5,
                format!("{} '{}' ({rid})", el.role, el.label),
            ));
        }

        if let Some(mid) = args.get("mark").and_then(|v| v.as_str()) {
            let Some(mark) = self.marks_by_id.get(mid).cloned() else {
                return Err(ToolOutput::json(err_json(
                    "stale_ref",
                    "Unknown or stale visual mark ID",
                )));
            };
            if mark.source != "ax"
                || mark.ax_ref.is_none()
                || !is_editable_role(&mark.role)
                || contains_secure_marker(&mark.label)
            {
                return Err(ToolOutput::json(err_json(
                    "target_identity_unverified",
                    &format!("{action_name} mark '{mid}' lacks a verified editable AX role"),
                )));
            }
            if let Some(ref axr) = mark.ax_ref {
                if let Some(el) = self.elements_by_ref.get(axr) {
                    if el.enabled == Some(false) {
                        return Err(ToolOutput::json(err_json(
                            "not_implemented",
                            "Element is disabled",
                        )));
                    }
                    if !is_editable_role(&el.role) {
                        return Err(ToolOutput::json(err_json(
                            "target_identity_unverified",
                            "Target role is not editable",
                        )));
                    }
                }
            }
            return Ok((
                app,
                mark.ax_ref.clone(),
                mark.center_x,
                mark.center_y,
                format!("mark {mid} ({} \"{}\")", mark.role, mark.label),
            ));
        }

        Err(ToolOutput::json(err_json(
            "invalid_argument",
            "Provide arguments.ref, arguments.mark, or arguments.target",
        )))
    }

    fn request_pointer_click(&mut self, action_name: &str, sub_args: &JsonValue, click_count: u8, button: &str, allow_coords: bool) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate(action_name, true) {
            return blocked;
        }
        let verify_opts = match parse_verify_options(sub_args, &self.config.timing) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let target_spec = match parse_semantic_target(sub_args) {
            Ok(s) => s,
            Err(e) => return e,
        };
        if let Some(spec) = target_spec {
            let default_app = self.snapshot_app.clone();
            let (app, el) = match self.resolve_semantic_target(default_app.as_deref(), sub_args, &spec) {
                Ok(v) => v,
                Err(e) => return e,
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
                ref_id: el.ref_id,
                label,
                click_count,
                button: button.to_string(),
            };
            return self.dispatch_pending(action_name, GatewayAction::Std(pending), &summary, verify_opts);
        }

        let Some(app) = self.snapshot_app.clone() else {
            return ToolOutput::json(err_json("stale_ref", "Inspect or visual_inspect an app or 'desktop' first"));
        };
        let has_coords = has_non_null(sub_args, "x") || has_non_null(sub_args, "y");
        if has_coords && self.snapshot_filtered {
            return ToolOutput::json(err_json("invalid_state", "Visual coordinate clicks require an unfiltered snapshot"));
        }

        if !has_non_null(sub_args, "mark") && !has_coords && has_non_null(sub_args, "ref") {
            let Some(rid) = sub_args.get("ref").and_then(|v| v.as_str()) else {
                return ToolOutput::json(err_json("invalid_argument", "'ref' must be a string"));
            };
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
            return self.dispatch_pending(action_name, GatewayAction::Std(pending), &summary, verify_opts);
        }

        let (x, y, prefer_ax, target_desc) = match self.resolve_point(sub_args, "ref", "mark", "x", "y") {
            Ok(res) => res,
            Err(e) => return e,
        };
        if has_coords && !allow_coords {
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
        self.dispatch_pending(action_name, GatewayAction::Std(pending), &summary, verify_opts)
    }

    fn request_hover(&mut self, sub_args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("hover", true) {
            return blocked;
        }
        let verify_opts = match parse_verify_options(sub_args, &self.config.timing) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let target_spec = match parse_semantic_target(sub_args) {
            Ok(s) => s,
            Err(e) => return e,
        };
        if let Some(spec) = target_spec {
            let default_app = self.snapshot_app.clone();
            let (app, el) = match self.resolve_semantic_target(default_app.as_deref(), sub_args, &spec) {
                Ok(v) => v,
                Err(e) => return e,
            };
            let Some(ref f) = el.frame else {
                return ToolOutput::json(err_json("target_identity_unverified", "Element lacks screen coordinates"));
            };
            let (x, y) = (f.x + f.w * 0.5, f.y + f.h * 0.5);
            let desc = format!("{} '{}' ({})", el.role, el.label, el.ref_id);
            return self.dispatch_pending(
                "hover",
                GatewayAction::Std(PendingAction::Hover { epoch: self.epoch, x, y }),
                &format!("Hover over {desc} in {app}. Ask the user before confirming."),
                verify_opts,
            );
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
            GatewayAction::Std(PendingAction::Hover { epoch: self.epoch, x, y }),
            &format!("Hover over {desc} in {app}. Ask the user before confirming."),
            verify_opts,
        )
    }

    fn request_drag(&mut self, sub_args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("drag", true) {
            return blocked;
        }
        let Some(app) = self.snapshot_app.clone() else {
            return ToolOutput::json(err_json("stale_ref", "Inspect first"));
        };
        let verify_opts = match parse_verify_options(sub_args, &self.config.timing) {
            Ok(v) => v,
            Err(e) => return e,
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
            GatewayAction::Std(PendingAction::Drag {
                epoch: self.epoch,
                x1,
                y1,
                x2,
                y2,
            }),
            &format!("Drag from {desc1} to {desc2} in {app}. Ask the user before confirming."),
            verify_opts,
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
                let (return_state, include_bounds_opt) = match parse_return_state_and_bounds(sub_args) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if self.audit.record(action, "allow", "requested").is_err() {
                    return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
                }
                self.invalidate_snapshot();
                self.snapshot_window = None;
                let res = if action == "activate_application" {
                    self.driver.activate_app(&bundle)
                } else {
                    self.driver.launch_app(&bundle)
                };
                match res {
                    Ok(()) => {
                        let mut fields = vec![
                            ("ok", JsonValue::Bool(true)),
                            ("app", JsonValue::String(bundle.clone())),
                        ];
                        if return_state != ReturnStateMode::None {
                            let eff_bounds = include_bounds_opt.unwrap_or(self.config.snapshot.include_bounds);
                            let active = self.attach_post_action_state(
                                &bundle,
                                None,
                                return_state,
                                eff_bounds,
                                &mut fields,
                            );
                            fields.push(("snapshot_preserved", JsonValue::Bool(active)));
                        } else {
                            fields.push(("hint", JsonValue::String("Inspect the app to verify its updated state.".into())));
                        }
                        ToolOutput::json(JsonValue::object(fields))
                    }
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
                let raw_app = match sub_args.get("app") {
                    None | Some(JsonValue::Null) => "com.google.Chrome",
                    Some(v) => match v.as_str() {
                        Some(s) => s,
                        None => return ToolOutput::json(err_json("invalid_argument", "'app' must be a string")),
                    },
                };
                let Some(bundle) = self.resolve_target(raw_app) else {
                    return ToolOutput::json(err_json("invalid_app", "App is invalid, protected, or not permitted"));
                };
                if let Err(e) = self.check_url_domain_allowed(url, "open_url") {
                    return e;
                }
                let (return_state, include_bounds_opt) = match parse_return_state_and_bounds(sub_args) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if self.audit.record("open_url", "allow", "requested").is_err() {
                    return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
                }
                self.invalidate_snapshot();
                self.snapshot_window = None;
                match self.driver.open_url(&bundle, url) {
                    Ok(()) => {
                        let mut fields = vec![
                            ("ok", JsonValue::Bool(true)),
                            ("app", JsonValue::String(bundle.clone())),
                            ("url", JsonValue::String(url.into())),
                        ];
                        if return_state != ReturnStateMode::None {
                            let eff_bounds = include_bounds_opt.unwrap_or(self.config.snapshot.include_bounds);
                            let active = self.attach_post_action_state(
                                &bundle,
                                None,
                                return_state,
                                eff_bounds,
                                &mut fields,
                            );
                            fields.push(("snapshot_preserved", JsonValue::Bool(active)));
                        }
                        ToolOutput::json(JsonValue::object(fields))
                    }
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
            "click" => self.request_pointer_click("click", sub_args, 1, "left", true),
            "double_click" => self.request_pointer_click("double_click", sub_args, 2, "left", true),
            "right_click" => self.request_pointer_click("right_click", sub_args, 1, "right", true),
            "hover" => self.request_hover(sub_args),
            "drag" => self.request_drag(sub_args),
            "type" => {
                if let Some(blocked) = self.preflight_gate("type", true) {
                    return blocked;
                }
                let Some(text) = sub_args.get("text").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing arguments.text"));
                };
                let mode = match sub_args.get("mode") {
                    None | Some(JsonValue::Null) => "insert",
                    Some(v) => v.as_str().unwrap_or(""),
                };
                if !matches!(mode, "insert" | "replace") {
                    return ToolOutput::json(err_json("invalid_argument", "mode must be 'insert' or 'replace'"));
                }
                if let Err(e) = validate_input_text(text, false) {
                    return e;
                }
                let verify_opts = match parse_verify_options(sub_args, &self.config.timing) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let target_spec = match parse_semantic_target(sub_args) {
                    Ok(s) => s,
                    Err(e) => return e,
                };

                let (app, ref_id, cx, cy, click_first) = if let Some(spec) = target_spec {
                    let default_app = self.snapshot_app.clone();
                    let (app, el) = match self.resolve_semantic_target(default_app.as_deref(), sub_args, &spec) {
                        Ok(v) => v,
                        Err(e) => return e,
                    };
                    let is_native_text = is_editable_role(&el.role);
                    if mode == "replace" && !is_native_text {
                        return ToolOutput::json(err_json("target_identity_unverified", "Replace mode requires a verified text field"));
                    }
                    let Some(ref f) = el.frame else {
                        return ToolOutput::json(err_json("target_identity_unverified", "Target lacks bounding frame"));
                    };
                    (app, Some(el.ref_id), f.x + f.w * 0.5, f.y + f.h * 0.5, !is_native_text)
                } else {
                    let Some(app) = self.snapshot_app.clone() else {
                        return ToolOutput::json(err_json("stale_ref", "Inspect an app first"));
                    };
                    if let Some(rid) = sub_args.get("ref").and_then(|v| v.as_str()) {
                        let Some(el) = self.elements_by_ref.get(rid) else {
                            return ToolOutput::json(err_json("stale_ref", "Unknown or stale element ref"));
                        };
                        let is_native_text = is_editable_role(&el.role);
                        if mode == "replace" && !is_native_text {
                            return ToolOutput::json(err_json("target_identity_unverified", "Replace mode requires a verified text field"));
                        }
                        let Some(ref f) = el.frame else {
                            return ToolOutput::json(err_json("target_identity_unverified", "Target lacks bounding frame"));
                        };
                        (app, Some(rid.to_string()), f.x + f.w * 0.5, f.y + f.h * 0.5, !is_native_text)
                    } else if let Some(mid) = sub_args.get("mark").and_then(|v| v.as_str()) {
                        let Some(mark) = self.marks_by_id.get(mid) else {
                            return ToolOutput::json(err_json("stale_ref", "Unknown or stale visual mark ID"));
                        };
                        if mode == "replace" {
                            return ToolOutput::json(err_json("invalid_argument", "Replace mode requires a verified Accessibility ref"));
                        }
                        (app, mark.ax_ref.clone(), mark.center_x, mark.center_y, true)
                    } else {
                        return ToolOutput::json(err_json("invalid_argument", "Provide arguments.ref, arguments.mark, or arguments.target"));
                    }
                };

                let summary = if mode == "insert" {
                    format!("Type {text:?} into {app}. Ask the user before confirming.")
                } else {
                    format!("Replace text field in {app} with {text:?}. Ask the user before confirming.")
                };
                self.dispatch_pending(
                    "type",
                    GatewayAction::Std(PendingAction::TypeText {
                        epoch: self.epoch,
                        ref_id,
                        x: cx,
                        y: cy,
                        text: text.to_string(),
                        mode: mode.to_string(),
                        click_first,
                    }),
                    &summary,
                    verify_opts,
                )
            }
            "set_value" => {
                if let Some(blocked) = self.preflight_gate("set_value", true) {
                    return blocked;
                }
                let Some(text) = sub_args.get("text").and_then(|v| v.as_str()) else {
                    return ToolOutput::json(err_json("invalid_argument", "Missing arguments.text"));
                };
                if let Err(e) = validate_input_text(text, true) {
                    return e;
                }
                let verify_opts = match parse_verify_options(sub_args, &self.config.timing) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let default_app = self.snapshot_app.clone();
                let (app, ref_id, cx, cy, target_desc) = match self.resolve_editable_target(
                    "set_value",
                    sub_args,
                    default_app.as_deref(),
                ) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let summary = format!(
                    "Set value on {target_desc} in {app} ({} chars). Ask the user before confirming.",
                    text.chars().count()
                );
                self.dispatch_pending(
                    "set_value",
                    GatewayAction::SetValue {
                        epoch: self.epoch,
                        ref_id,
                        x: cx,
                        y: cy,
                        text: text.to_string(),
                    },
                    &summary,
                    verify_opts,
                )
            }
            "select_text" => {
                if let Some(blocked) = self.preflight_gate("select_text", true) {
                    return blocked;
                }
                let (start, end) = match parse_select_range(sub_args) {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                let verify_opts = match parse_verify_options(sub_args, &self.config.timing) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let default_app = self.snapshot_app.clone();
                let (app, ref_id, cx, cy, target_desc) = match self.resolve_editable_target(
                    "select_text",
                    sub_args,
                    default_app.as_deref(),
                ) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let summary = format!("Select text on {target_desc} in {app}. Ask the user before confirming.");
                self.dispatch_pending(
                    "select_text",
                    GatewayAction::SelectText {
                        epoch: self.epoch,
                        ref_id,
                        x: cx,
                        y: cy,
                        start,
                        end,
                    },
                    &summary,
                    verify_opts,
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
                let verify_opts = match parse_verify_options(sub_args, &self.config.timing) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
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
                    GatewayAction::Std(PendingAction::PressKey {
                        epoch: self.epoch,
                        key: key.to_string(),
                        modifiers: mods,
                    }),
                    &summary,
                    verify_opts,
                )
            }
            "scroll" => {
                if let Some(blocked) = self.preflight_gate("scroll", true) {
                    return blocked;
                }
                let verify_opts = match parse_verify_options(sub_args, &self.config.timing) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let target_spec = match parse_semantic_target(sub_args) {
                    Ok(s) => s,
                    Err(e) => return e,
                };
                let (app, direction, amount, target_pt, desc) = if let Some(spec) = target_spec {
                    let (dir, amt) = match parse_scroll_dir_amount(sub_args) {
                        Ok(v) => v,
                        Err(e) => return e,
                    };
                    let default_app = self.snapshot_app.clone();
                    let (resolved_app, el) = match self.resolve_semantic_target(default_app.as_deref(), sub_args, &spec) {
                        Ok(v) => v,
                        Err(e) => return e,
                    };
                    let Some(ref f) = el.frame else {
                        return ToolOutput::json(err_json("target_identity_unverified", "Element lacks coordinates"));
                    };
                    let (px, py) = (f.x + f.w * 0.5, f.y + f.h * 0.5);
                    (
                        resolved_app,
                        dir,
                        amt,
                        Some((px, py)),
                        format!("{} '{}' ({})", el.role, el.label, el.ref_id),
                    )
                } else {
                    let Some(app) = self.snapshot_app.clone() else {
                        return ToolOutput::json(err_json("stale_ref", "Inspect an app first"));
                    };
                    let (dir, amt, pt, desc) = match resolve_scroll_args(
                        sub_args,
                        &self.elements_by_ref,
                        &self.marks_by_id,
                        self.snapshot_filtered,
                    ) {
                        Ok(v) => v,
                        Err(e) => return e,
                    };
                    (app, dir, amt, pt, desc)
                };
                let (x, y) = match target_pt {
                    Some((px, py)) => (Some(px), Some(py)),
                    None => (None, None),
                };
                let summary = if desc.is_empty() {
                    format!("Scroll {direction} {amount} lines in {app}. Ask the user before confirming.")
                } else {
                    format!("Scroll {direction} {amount} lines at {desc} in {app}. Ask the user before confirming.")
                };
                self.dispatch_pending(
                    "scroll",
                    GatewayAction::Std(PendingAction::Scroll {
                        epoch: self.epoch,
                        direction,
                        amount,
                        x,
                        y,
                    }),
                    &summary,
                    verify_opts,
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
                let verify_opts = self
                    .pending_verify
                    .remove(token)
                    .unwrap_or_else(|| ActionVerifyOptions::from_timing(&self.config.timing));
                let gw_action = match self.pending_gateway_actions.remove(token) {
                    Some(gw) => gw,
                    None => {
                        if matches!(
                            &approved,
                            PendingAction::TypeText { mode, .. }
                                if mode == "set_value" || mode == "select_text"
                        ) {
                            return ToolOutput::json(err_json(
                                "invalid_confirmation",
                                "Invalid or expired confirmation token",
                            ));
                        }
                        GatewayAction::Std(approved)
                    }
                };
                self.execute_action(gw_action, "confirmed", verify_opts)
            }
            _ => ToolOutput::json(err_json("not_implemented", "Operation disabled")),
        }
    }

    fn action_destructive_label(&self, action: &GatewayAction) -> Option<String> {
        match action {
            GatewayAction::Std(PendingAction::ClickRef { label, .. }) if is_destructive_label(label) => {
                Some(label.clone())
            }
            GatewayAction::Std(PendingAction::VisualClick {
                mark_id: Some(mid), ..
            }) => self
                .marks_by_id
                .get(mid)
                .filter(|m| is_destructive_label(&m.label))
                .map(|m| m.label.clone()),
            _ => None,
        }
    }

    fn dispatch_pending(
        &mut self,
        action_name: &str,
        action: GatewayAction,
        summary: &str,
        verify_opts: ActionVerifyOptions,
    ) -> ToolOutput {
        let needs_destructive_confirm =
            self.settings.confirm_destructive && self.action_destructive_label(&action).is_some();
        if !self.settings.require_confirmation && !needs_destructive_confirm {
            return self.execute_action(action, "direct", verify_opts);
        }
        if self.audit.record(action_name, "confirm", "confirmation_required").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }
        let std_action = match &action {
            GatewayAction::Std(p) => p.clone(),
            GatewayAction::SetValue {
                epoch,
                ref_id,
                x,
                y,
                text,
            } => PendingAction::TypeText {
                epoch: *epoch,
                ref_id: ref_id.clone(),
                x: *x,
                y: *y,
                text: text.clone(),
                mode: "set_value".into(),
                click_first: false,
            },
            GatewayAction::SelectText {
                epoch,
                ref_id,
                x,
                y,
                ..
            } => PendingAction::TypeText {
                epoch: *epoch,
                ref_id: ref_id.clone(),
                x: *x,
                y: *y,
                text: String::new(),
                mode: "select_text".into(),
                click_first: false,
            },
        };
        let token = self.confirmations.issue(std_action);
        self.confirmations.evict_excess(256);
        self.pending_verify
            .retain(|k, _| self.confirmations.contains(k));
        self.pending_gateway_actions
            .retain(|k, _| self.confirmations.contains(k));
        self.pending_verify.insert(token.clone(), verify_opts);
        if !matches!(action, GatewayAction::Std(_)) {
            self.pending_gateway_actions.insert(token.clone(), action);
        }
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

    fn fresh_has_editable_near(&self, fresh: &AppSnapshot, x: f64, y: f64) -> bool {
        fresh.elements.iter().any(|el| {
            let Some(ref f) = el.frame else {
                return false;
            };
            is_editable_role(&el.role)
                && ((f.x + f.w * 0.5) - x).abs() <= 24.0
                && ((f.y + f.h * 0.5) - y).abs() <= 24.0
        })
    }

    fn snapshot_matches_action(&self, fresh: &AppSnapshot, action: &GatewayAction) -> bool {
        if self.snapshot_pid.is_some_and(|p| p != fresh.pid) {
            return false;
        }
        let fresh_sheets = fresh.elements.iter().filter(|e| e.role == "Sheet").count();
        if fresh_sheets > self.snapshot_sheets {
            return false;
        }
        match action {
            GatewayAction::Std(PendingAction::ClickRef { ref_id, label, .. }) => {
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
            GatewayAction::Std(PendingAction::VisualClick { mark_id, x, y, .. }) => {
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
            GatewayAction::Std(PendingAction::TypeText { ref_id, x, y, click_first, .. }) => {
                if ref_id.is_some() {
                    if *click_first {
                        return self.fresh_has_element_near(fresh, *x, *y, None, None);
                    }
                    return self.fresh_has_editable_near(fresh, *x, *y);
                }
                true
            }
            GatewayAction::SetValue { x, y, .. } | GatewayAction::SelectText { x, y, .. } => {
                self.fresh_has_editable_near(fresh, *x, *y)
            }
            _ => true,
        }
    }

    fn verify_points_safe(&self, app: &str, pts: &[(f64, f64)]) -> Result<(), ToolOutput> {
        self.verify_points_with_identity(app, pts, &ExpectedPointTarget::Any)
    }

    fn verify_points_with_identity(
        &self,
        app: &str,
        pts: &[(f64, f64)],
        expected: &ExpectedPointTarget,
    ) -> Result<(), ToolOutput> {
        if !self.driver.supports_fast_point_verify() {
            return Ok(());
        }
        if !matches!(expected, ExpectedPointTarget::Any) {
            if let Ok(ws) = self.driver.window_state(app) {
                if self.snapshot_ws_pid.is_some_and(|p| p != ws.pid) {
                    return Err(ToolOutput::json(err_json(
                        "stale_ref",
                        "Target process changed before input; inspect again",
                    )));
                }
                if self.snapshot_ws_windows_total.is_some_and(|n| ws.windows_total > n) {
                    return Err(ToolOutput::json(err_json(
                        "stale_ref",
                        "A new window opened since snapshot; inspect again",
                    )));
                }
            }
        }
        for &(px, py) in pts {
            match self.driver.probe_point_safe(app, px, py) {
                Ok(probe) => {
                    if !probe.safe {
                        return Err(ToolOutput::json(err_json(
                            "unsafe_target_field",
                            "Target point became unsafe or left window bounds",
                        )));
                    }
                    if !point_probe_matches_expected(&probe, expected) {
                        return Err(ToolOutput::json(err_json(
                            "stale_ref",
                            "Target element changed or shifted before input; inspect again",
                        )));
                    }
                }
                Err(e) => return Err(ToolOutput::json(err_json(&e.code, &e.message))),
            }
        }
        Ok(())
    }

    fn capture_verify_baseline(
        &self,
        app: &str,
        cached_ax: Option<&str>,
        mode: VerifyMode,
    ) -> (Option<String>, Option<String>) {
        let ax = if mode == VerifyMode::Visual {
            None
        } else {
            cached_ax.map(str::to_string).or_else(|| {
                self.driver.inspect_app(app, None, 150, 12).ok().and_then(|s| {
                    if self.check_frontmost_permitted(&s.frontmost_bundle_id)
                        && snapshot_redaction_reason(&s).is_none()
                    {
                        let out = s.render_outline(None);
                        if !contains_secure_marker(&out) {
                            return Some(s.comparable_text());
                        }
                    }
                    None
                })
            })
        };
        let digest = if mode == VerifyMode::Ax {
            None
        } else {
            self.driver
                .capture_and_ocr(app, None, OcrMode::Off, false, &ImageEncodeOptions::default())
                .ok()
                .and_then(|c| c.pixel_digest)
        };
        (ax, digest)
    }

    fn capture_verify_after(
        &self,
        app: &str,
        mode: VerifyMode,
    ) -> Result<(Option<String>, Option<String>), ToolOutput> {
        let ax = if mode == VerifyMode::Visual {
            None
        } else {
            match self.driver.inspect_app(app, None, 150, 12) {
                Ok(s) => {
                    if !self.check_frontmost_permitted(&s.frontmost_bundle_id) {
                        return Err(ToolOutput::json(err_json("invalid_app", "Frontmost application is protected")));
                    }
                    if let Some(reason) = snapshot_redaction_reason(&s) {
                        return Err(ToolOutput::json(err_json("unsafe_or_incomplete_tree", &reason)));
                    }
                    let out = s.render_outline(None);
                    if contains_secure_marker(&out) {
                        return Err(ToolOutput::json(err_json("unsafe_or_incomplete_tree", "sensitive_text_marker")));
                    }
                    Some(s.comparable_text())
                }
                Err(_) => None,
            }
        };
        let digest = if mode == VerifyMode::Ax {
            None
        } else {
            self.driver
                .capture_and_ocr(app, None, OcrMode::Off, false, &ImageEncodeOptions::default())
                .ok()
                .and_then(|c| c.pixel_digest)
        };
        Ok((ax, digest))
    }

    fn execute_verified_request(
        &self,
        app: &str,
        recheck_fast_points: bool,
        cached_ax: Option<&str>,
        safety_pts: &[(f64, f64)],
        req: &ActionRequest,
        opts: &ActionVerifyOptions,
    ) -> Result<(String, Option<VerifyTelemetry>), ToolOutput> {
        if !opts.verify_change {
            let res = self
                .driver
                .perform_action(req)
                .map_err(|e| ToolOutput::json(err_json(&e.code, &e.message)))?;
            return Ok((res.method, None));
        }

        let (before_ax, before_digest) =
            self.capture_verify_baseline(app, cached_ax, opts.verify_mode);
        let settle_dur = Duration::from_millis(opts.settle_ms);
        let poll_dur = Duration::from_millis(opts.poll_interval_ms.max(1));

        for attempt in 0..=opts.max_retries {
            if self.stop_file.exists() {
                return Err(ToolOutput::json(err_json("panic_stop", "STOP file present")));
            }
            if attempt > 0 {
                thread::sleep(Duration::from_millis(opts.retry_delay_ms));
                self.driver.tick();
                if recheck_fast_points {
                    self.verify_points_safe(app, safety_pts)?;
                }
            }
            let res = self
                .driver
                .perform_action(req)
                .map_err(|e| ToolOutput::json(err_json(&e.code, &e.message)))?;

            let attempt_start = Instant::now();
            let (ax_changed, visual_changed) = loop {
                let elapsed = attempt_start.elapsed();
                let rem = settle_dur.saturating_sub(elapsed);
                let sleep_for = if rem.is_zero() {
                    Duration::ZERO
                } else {
                    rem.min(poll_dur)
                };
                if !sleep_for.is_zero() {
                    thread::sleep(sleep_for);
                }
                if self.stop_file.exists() {
                    return Err(ToolOutput::json(err_json("panic_stop", "STOP file present")));
                }
                self.driver.tick();

                let (after_ax, after_digest) =
                    self.capture_verify_after(app, opts.verify_mode)?;
                let ax_changed = matches!((&before_ax, &after_ax), (Some(b), Some(a)) if b != a);
                let visual_changed = matches!(
                    (&before_digest, &after_digest),
                    (Some(b), Some(a)) if visual_digest_changed(b, a)
                );
                if ax_changed || visual_changed || attempt_start.elapsed() >= settle_dur {
                    break (ax_changed, visual_changed);
                }
            };

            let changed = ax_changed || visual_changed;
            if changed || attempt == opts.max_retries {
                return Ok((
                    res.method,
                    Some(VerifyTelemetry {
                        verified_change: changed,
                        ax_changed,
                        visual_changed,
                        attempts: attempt + 1,
                        retried: attempt,
                    }),
                ));
            }
        }
        unreachable!("loop terminates at attempt == max_retries")
    }

    fn execute_action(
        &mut self,
        action: GatewayAction,
        policy_label: &str,
        verify_opts: ActionVerifyOptions,
    ) -> ToolOutput {
        let Some(app) = self.snapshot_app.clone() else {
            return ToolOutput::json(err_json("stale_ref", "No active snapshot"));
        };
        let action_epoch = match &action {
            GatewayAction::Std(PendingAction::ClickRef { epoch, .. })
            | GatewayAction::Std(PendingAction::VisualClick { epoch, .. })
            | GatewayAction::Std(PendingAction::Hover { epoch, .. })
            | GatewayAction::Std(PendingAction::Drag { epoch, .. })
            | GatewayAction::Std(PendingAction::TypeText { epoch, .. })
            | GatewayAction::Std(PendingAction::PressKey { epoch, .. })
            | GatewayAction::Std(PendingAction::Scroll { epoch, .. })
            | GatewayAction::SetValue { epoch, .. }
            | GatewayAction::SelectText { epoch, .. } => *epoch,
        };
        if action_epoch != self.epoch {
            return ToolOutput::json(err_json("stale_ref", "Snapshot changed before confirmation"));
        }

        let safety_pts = match &action {
            GatewayAction::Std(PendingAction::ClickRef { ref_id, .. }) => {
                let Some(pts) = self
                    .elements_by_ref
                    .get(ref_id)
                    .and_then(|el| el.frame.as_ref())
                    .map(|f| vec![(f.x + f.w * 0.5, f.y + f.h * 0.5)])
                else {
                    self.invalidate_snapshot();
                    return ToolOutput::json(err_json("stale_ref", "Element ref or frame missing"));
                };
                pts
            }
            GatewayAction::Std(PendingAction::VisualClick { x, y, .. })
            | GatewayAction::Std(PendingAction::Hover { x, y, .. })
            | GatewayAction::Std(PendingAction::TypeText { x, y, .. })
            | GatewayAction::SetValue { x, y, .. }
            | GatewayAction::SelectText { x, y, .. } => vec![(*x, *y)],
            GatewayAction::Std(PendingAction::Drag { x1, y1, x2, y2, .. }) => {
                vec![(*x1, *y1), (*x2, *y2)]
            }
            GatewayAction::Std(PendingAction::PressKey { .. }) => Vec::new(),
            GatewayAction::Std(PendingAction::Scroll {
                x: Some(px),
                y: Some(py),
                ..
            }) => vec![(*px, *py)],
            GatewayAction::Std(PendingAction::Scroll { .. }) => Vec::new(),
        };

        let fast_direct = policy_label == "direct" && self.driver.supports_fast_point_verify();
        if fast_direct {
            let expected = expected_target_for_action(&action, &self.elements_by_ref, &self.marks_by_id);
            if let Err(err_out) = self.verify_points_with_identity(&app, &safety_pts, &expected) {
                self.invalidate_snapshot();
                return err_out;
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

        let is_non_navigating = matches!(
            &action,
            GatewayAction::Std(PendingAction::TypeText { .. })
                | GatewayAction::SetValue { .. }
                | GatewayAction::SelectText { .. }
        );
        let action_name = match &action {
            GatewayAction::Std(PendingAction::ClickRef { click_count: 2, .. })
            | GatewayAction::Std(PendingAction::VisualClick { click_count: 2, .. }) => {
                "double_click"
            }
            GatewayAction::Std(PendingAction::ClickRef { button, .. })
            | GatewayAction::Std(PendingAction::VisualClick { button, .. })
                if button == "right" =>
            {
                "right_click"
            }
            GatewayAction::Std(PendingAction::ClickRef { .. }) => "click",
            GatewayAction::Std(PendingAction::VisualClick { .. }) => "visual_click",
            GatewayAction::Std(PendingAction::Hover { .. }) => "hover",
            GatewayAction::Std(PendingAction::Drag { .. }) => "drag",
            GatewayAction::Std(PendingAction::TypeText { .. }) => "type",
            GatewayAction::SetValue { .. } => "set_value",
            GatewayAction::SelectText { .. } => "select_text",
            GatewayAction::Std(PendingAction::PressKey { .. }) => "press_key",
            GatewayAction::Std(PendingAction::Scroll { .. }) => "scroll",
        };
        if self.audit.record(action_name, policy_label, "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }

        let driver_req = match action {
            GatewayAction::Std(PendingAction::ClickRef {
                ref_id,
                click_count,
                button,
                ..
            }) => {
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
            GatewayAction::Std(PendingAction::VisualClick {
                x,
                y,
                prefer_ax,
                click_count,
                button,
                ..
            }) => ActionRequest::ClickPoint {
                bundle_id: app.clone(),
                x,
                y,
                prefer_ax,
                click_count,
                button,
            },
            GatewayAction::Std(PendingAction::Hover { x, y, .. }) => ActionRequest::HoverPoint {
                bundle_id: app.clone(),
                x,
                y,
            },
            GatewayAction::Std(PendingAction::Drag { x1, y1, x2, y2, .. }) => {
                ActionRequest::DragPoints {
                    bundle_id: app.clone(),
                    x1,
                    y1,
                    x2,
                    y2,
                }
            }
            GatewayAction::Std(PendingAction::TypeText {
                x,
                y,
                text,
                mode,
                click_first,
                ..
            }) => ActionRequest::TypePoint {
                bundle_id: app.clone(),
                x,
                y,
                text,
                mode,
                click_first,
                key_delay_us: self.config.timing.key_delay_us,
            },
            GatewayAction::SetValue { x, y, text, .. } => ActionRequest::SetValue {
                bundle_id: app.clone(),
                x,
                y,
                text,
                key_delay_us: self.config.timing.key_delay_us,
            },
            GatewayAction::SelectText {
                x, y, start, end, ..
            } => ActionRequest::SelectText {
                bundle_id: app.clone(),
                x,
                y,
                start,
                end,
            },
            GatewayAction::Std(PendingAction::PressKey { key, modifiers, .. }) => {
                ActionRequest::PressKey {
                    bundle_id: app.clone(),
                    key,
                    modifiers,
                }
            }
            GatewayAction::Std(PendingAction::Scroll {
                direction,
                amount,
                x,
                y,
                ..
            }) => ActionRequest::Scroll {
                bundle_id: app.clone(),
                direction,
                amount,
                x,
                y,
            },
        };

        let post_window = self.snapshot_window;
        let cached_ax = if self.snapshot_window.is_none() {
            self.snapshot_comparable.clone()
        } else {
            None
        };
        if !is_non_navigating {
            self.invalidate_snapshot();
        }
        match self.execute_verified_request(
            &app,
            fast_direct,
            cached_ax.as_deref(),
            &safety_pts,
            &driver_req,
            &verify_opts,
        ) {
            Ok((method, telemetry)) => {
                if verify_opts.require_change && telemetry.is_some_and(|t| !t.verified_change) {
                    self.invalidate_snapshot();
                    let mut fields = vec![
                        ("ok", JsonValue::Bool(false)),
                        ("code", JsonValue::String("no_change_detected".into())),
                        ("message", JsonValue::String("Action produced no detectable Accessibility or pixel change".into())),
                        ("policy", JsonValue::String(policy_label.into())),
                        ("completed", JsonValue::Bool(false)),
                        ("method", JsonValue::String(method)),
                    ];
                    if let Some(t) = telemetry {
                        append_verify_fields(&mut fields, &t);
                    }
                    return ToolOutput::json(JsonValue::object(fields));
                }
                let hint = match telemetry {
                    Some(t) if t.verified_change => "UI change verified via Accessibility or visual digest.",
                    Some(_) => "No AX or visual change detected after retrying; verify target or inspect state.",
                    None if verify_opts.return_state != ReturnStateMode::None => {
                        "Updated AX state returned inline."
                    }
                    None => "Verify the resulting state via inspect_app or visual_inspect.",
                };
                let mut fields = vec![
                    ("ok", JsonValue::Bool(true)),
                    ("policy", JsonValue::String(policy_label.into())),
                    ("completed", JsonValue::Bool(false)),
                    ("method", JsonValue::String(method)),
                    ("hint", JsonValue::String(hint.into())),
                ];
                let snapshot_preserved = if verify_opts.return_state != ReturnStateMode::None {
                    let eff_bounds = verify_opts
                        .include_bounds
                        .unwrap_or(self.config.snapshot.include_bounds);
                    Some(self.attach_post_action_state(
                        &app,
                        post_window,
                        verify_opts.return_state,
                        eff_bounds,
                        &mut fields,
                    ))
                } else if is_non_navigating {
                    let keep = verify_opts
                        .keep_snapshot
                        .unwrap_or(self.config.snapshot.reuse_form_refs);
                    if keep && self.should_preserve_snapshot(&app) {
                        self.snapshot_comparable = None;
                        Some(true)
                    } else {
                        self.invalidate_snapshot();
                        Some(false)
                    }
                } else {
                    None
                };
                if let Some(preserved) = snapshot_preserved {
                    fields.push(("snapshot_preserved", JsonValue::Bool(preserved)));
                }
                if let Some(t) = telemetry {
                    append_verify_fields(&mut fields, &t);
                }
                ToolOutput::json(JsonValue::object(fields))
            }
            Err(err_out) => {
                self.invalidate_snapshot();
                err_out
            }
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
                        verify_change: item
                            .get("verify_change")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false),
                    })
                    .collect();
                let expected_domain = args.get("expected_domain").and_then(|v| v.as_str());
                let expected_window_contains =
                    args.get("expected_window_contains").and_then(|v| v.as_str());
                match self.memory.record_with_preconditions(
                    &bundle,
                    goal,
                    expected_domain,
                    expected_window_contains,
                    steps,
                ) {
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
                if !valid_app_id(raw_app) {
                    return ToolOutput::json(err_json("invalid_app", "Invalid app ID"));
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
        if steps.is_empty() || steps.len() > 24 {
            return ToolOutput::json(err_json("invalid_argument", "steps must contain 1..=24 items"));
        }
        let inspect_after = match parse_optional_bool(sub_args, "inspect_after", false) {
            Ok(b) => b,
            Err(e) => return e,
        };
        let batch_opts = match parse_verify_options(sub_args, &self.config.timing) {
            Ok(v) => v,
            Err(e) => return e,
        };
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
        if has_non_null(sub_args, "app") && self.snapshot_app.as_deref() != Some(cur_app.as_str()) {
            self.invalidate_snapshot();
            self.snapshot_window = None;
        }

        let initial_ax = self.snapshot_comparable.clone();
        let initial_app = self.snapshot_app.clone();
        let mut step_filtered = self.snapshot_filtered;
        let initial_window_unscoped = self.snapshot_window.is_none();
        let mut navigated = false;
        let mut step_results = Vec::with_capacity(steps.len());

        for (idx, step) in steps.iter().enumerate() {
            match self.execute_batch_step(
                idx,
                step,
                &batch_opts,
                &mut cur_app,
                &mut step_filtered,
                &mut navigated,
                initial_ax.as_deref(),
                &initial_app,
                initial_window_unscoped,
            ) {
                Ok(item) => step_results.push(item),
                Err((err_out, partial_item)) => {
                    if let Some(item) = partial_item {
                        step_results.push(item);
                    }
                    self.invalidate_snapshot();
                    return batch_failure_output(err_out, idx, step_results);
                }
            }
        }

        let eff_return_state = if batch_opts.return_state != ReturnStateMode::None {
            batch_opts.return_state
        } else if inspect_after {
            if self.config.snapshot.ax_diff {
                ReturnStateMode::Diff
            } else {
                ReturnStateMode::Full
            }
        } else {
            ReturnStateMode::None
        };
        let eff_bounds = batch_opts
            .include_bounds
            .unwrap_or(self.config.snapshot.include_bounds);

        let mut out = vec![
            ("ok", JsonValue::Bool(true)),
            ("policy", JsonValue::String("direct".into())),
            ("executed", JsonValue::Number(step_results.len() as f64)),
            ("results", JsonValue::Array(step_results)),
        ];
        if eff_return_state != ReturnStateMode::None {
            let post_window = if self.snapshot_app.as_deref() == Some(cur_app.as_str()) {
                self.snapshot_window
            } else {
                None
            };
            let active = self.attach_post_action_state(
                &cur_app,
                post_window,
                eff_return_state,
                eff_bounds,
                &mut out,
            );
            out.push(("snapshot_preserved", JsonValue::Bool(active)));
        } else if !navigated
            && batch_opts
                .keep_snapshot
                .unwrap_or(self.config.snapshot.reuse_form_refs)
            && self.should_preserve_snapshot(&cur_app)
        {
            self.snapshot_comparable = None;
        } else {
            self.invalidate_snapshot();
        }
        ToolOutput::json(JsonValue::object(out))
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_batch_step(
        &mut self,
        idx: usize,
        step: &JsonValue,
        batch_opts: &ActionVerifyOptions,
        cur_app: &mut String,
        step_filtered: &mut bool,
        navigated: &mut bool,
        initial_ax: Option<&str>,
        initial_app: &Option<String>,
        initial_window_unscoped: bool,
    ) -> Result<JsonValue, (ToolOutput, Option<JsonValue>)> {
        let step_err = |err: ToolOutput| (err, None);
        if self.stop_file.exists() {
            return Err(step_err(ToolOutput::json(err_json("panic_stop", "STOP file present"))));
        }
        let Some(act) = step.get("action").and_then(|v| v.as_str()) else {
            return Err(step_err(ToolOutput::json(err_json(
                "invalid_argument",
                &format!("Step {idx} missing action"),
            ))));
        };

        if act == "sleep_ms" {
            let ms = step.get("ms").and_then(|v| v.as_i64()).unwrap_or(200).clamp(10, 3000) as u64;
            thread::sleep(Duration::from_millis(ms));
            self.driver.tick();
            return Ok(JsonValue::object(vec![
                ("step", JsonValue::Number(idx as f64)),
                ("action", JsonValue::String("sleep_ms".into())),
                ("ok", JsonValue::Bool(true)),
            ]));
        }
        if act == "activate_application" {
            let Some(raw) = step.get("app").and_then(|v| v.as_str()) else {
                return Err(step_err(ToolOutput::json(err_json(
                    "invalid_argument",
                    &format!("Step {idx} missing app"),
                ))));
            };
            let Some(bundle) = self.resolve_target(raw) else {
                return Err(step_err(ToolOutput::json(err_json(
                    "invalid_app",
                    "Target is invalid, protected, or not permitted",
                ))));
            };
            if let Err(e) = self.driver.activate_app(&bundle) {
                return Err(step_err(ToolOutput::json(err_json(&e.code, &e.message))));
            }
            self.invalidate_snapshot();
            self.snapshot_window = None;
            self.snapshot_app = Some(bundle.clone());
            *cur_app = bundle.clone();
            *step_filtered = false;
            *navigated = true;
            return Ok(JsonValue::object(vec![
                ("step", JsonValue::Number(idx as f64)),
                ("action", JsonValue::String("activate_application".into())),
                ("app", JsonValue::String(bundle)),
                ("ok", JsonValue::Bool(true)),
            ]));
        }
        if act == "inspect" {
            let target = if let Some(raw) = step.get("app").and_then(|v| v.as_str()) {
                let Some(resolved) = self.resolve_target(raw) else {
                    return Err(step_err(ToolOutput::json(err_json(
                        "invalid_app",
                        "Target is invalid, protected, or not permitted",
                    ))));
                };
                resolved
            } else {
                cur_app.clone()
            };
            let window = parse_optional_window(step).map_err(step_err)?;
            let query = parse_optional_query(step).map_err(step_err)?;
            let diff_enabled =
                parse_optional_bool(step, "diff", self.config.snapshot.ax_diff).map_err(step_err)?;
            let include_bounds = parse_optional_bool(
                step,
                "include_bounds",
                batch_opts
                    .include_bounds
                    .unwrap_or(self.config.snapshot.include_bounds),
            )
            .map_err(step_err)?;
            self.invalidate_snapshot();
            let snap = match self.driver.inspect_app(&target, window, 150, 12) {
                Ok(s) => s,
                Err(e) => {
                    self.prev_ax_snapshot = None;
                    self.prev_ax_window = None;
                    return Err(step_err(ToolOutput::json(err_json(&e.code, &e.message))));
                }
            };
            if let Err(redaction) = self.check_ax_snapshot_safety(&snap) {
                return Err(step_err(redaction_to_error(redaction)));
            }
            let raw_comparable = snap.comparable_text();
            let (content, state_mode, reconciled_snap) = self.finalize_ax_outline_and_baseline(
                snap,
                window,
                query,
                diff_enabled,
                include_bounds,
            );
            self.record_snapshot_meta(&reconciled_snap, window, 150, 12, query.is_some());
            self.snapshot_comparable = Some(raw_comparable);
            *cur_app = reconciled_snap.bundle_id;
            *step_filtered = self.snapshot_filtered;
            *navigated = false;
            return Ok(JsonValue::object(vec![
                ("step", JsonValue::Number(idx as f64)),
                ("action", JsonValue::String("inspect".into())),
                ("ok", JsonValue::Bool(true)),
                ("state_mode", JsonValue::String(state_mode.into())),
                ("content", JsonValue::String(content)),
            ]));
        }
        if act == "wait_for" {
            let wait_payload = self.wait_for_condition(step, Some(cur_app)).map_err(step_err)?;
            if wait_payload.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                if let Some(a) = self.snapshot_app.clone() {
                    *cur_app = a;
                }
                *step_filtered = self.snapshot_filtered;
                *navigated = false;
                let src = wait_payload
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ax")
                    .to_string();
                let elapsed = wait_payload
                    .get("elapsed_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                return Ok(JsonValue::object(vec![
                    ("step", JsonValue::Number(idx as f64)),
                    ("action", JsonValue::String("wait_for".into())),
                    ("ok", JsonValue::Bool(true)),
                    ("matched", JsonValue::Bool(true)),
                    ("source", JsonValue::String(src)),
                    ("elapsed_ms", JsonValue::Number(elapsed)),
                ]));
            }
            let code = wait_payload
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("wait_timeout")
                .to_string();
            let msg = wait_payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("wait_for timed out")
                .to_string();
            return Err(step_err(ToolOutput::json(err_json(&code, &msg))));
        }

        let mut verify_opts = parse_verify_options(step, &self.config.timing).map_err(step_err)?;
        if !has_non_null(step, "verify_mode") {
            verify_opts.verify_mode = batch_opts.verify_mode;
        }

        let (req, safety_pts, expected_target) = match act {
            "click" | "double_click" | "right_click" | "hover" => {
                *navigated = true;
                let target_spec = parse_semantic_target(step).map_err(step_err)?;
                let (x, y, prefer_ax, exp) = if let Some(spec) = target_spec {
                    let (resolved_app, el) = self
                        .resolve_semantic_target(Some(cur_app), step, &spec)
                        .map_err(step_err)?;
                    *cur_app = resolved_app;
                    *step_filtered = self.snapshot_filtered;
                    if el.enabled == Some(false) {
                        return Err(step_err(ToolOutput::json(err_json(
                            "not_implemented",
                            "Element is disabled",
                        ))));
                    }
                    let Some(ref f) = el.frame else {
                        return Err(step_err(ToolOutput::json(err_json(
                            "target_identity_unverified",
                            "Element lacks coordinates",
                        ))));
                    };
                    let exp = expected_target_for_element(&el);
                    (f.x + f.w * 0.5, f.y + f.h * 0.5, supports_ax_press(&el), exp)
                } else {
                    let (px, py, pax) =
                        resolve_batch_point(step, &self.elements_by_ref, &self.marks_by_id)
                            .map_err(step_err)?;
                    let exp = expected_target_from_args(step, &self.elements_by_ref, &self.marks_by_id);
                    (px, py, pax, exp)
                };
                let req = if act == "hover" {
                    ActionRequest::HoverPoint {
                        bundle_id: cur_app.clone(),
                        x,
                        y,
                    }
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
                (req, vec![(x, y)], exp)
            }
            "drag" => {
                *navigated = true;
                let (x1, y1, _, _) = resolve_point_from_maps(
                    step,
                    &self.elements_by_ref,
                    &self.marks_by_id,
                    "from_ref",
                    "from_mark",
                    "x1",
                    "y1",
                    false,
                )
                .map_err(step_err)?;
                let (x2, y2, _, _) = resolve_point_from_maps(
                    step,
                    &self.elements_by_ref,
                    &self.marks_by_id,
                    "to_ref",
                    "to_mark",
                    "x2",
                    "y2",
                    false,
                )
                .map_err(step_err)?;
                (
                    ActionRequest::DragPoints {
                        bundle_id: cur_app.clone(),
                        x1,
                        y1,
                        x2,
                        y2,
                    },
                    vec![(x1, y1), (x2, y2)],
                    ExpectedPointTarget::Any,
                )
            }
            "type" => {
                let Some(text) = step.get("text").and_then(|v| v.as_str()) else {
                    return Err(step_err(ToolOutput::json(err_json(
                        "invalid_argument",
                        &format!("Step {idx} missing text"),
                    ))));
                };
                validate_input_text(text, false).map_err(step_err)?;
                let mode = match step.get("mode") {
                    None | Some(JsonValue::Null) => "insert",
                    Some(v) => v.as_str().unwrap_or(""),
                };
                if !matches!(mode, "insert" | "replace") {
                    return Err(step_err(ToolOutput::json(err_json(
                        "invalid_argument",
                        "mode must be 'insert' or 'replace'",
                    ))));
                }
                let target_spec = parse_semantic_target(step).map_err(step_err)?;
                let (x, y, default_click_first, exp) = if let Some(spec) = target_spec {
                    let (resolved_app, el) = self
                        .resolve_semantic_target(Some(cur_app), step, &spec)
                        .map_err(step_err)?;
                    *cur_app = resolved_app;
                    *step_filtered = self.snapshot_filtered;
                    let is_native = is_editable_role(&el.role);
                    if mode == "replace" && !is_native {
                        return Err(step_err(ToolOutput::json(err_json(
                            "target_identity_unverified",
                            "Replace mode requires a verified text field",
                        ))));
                    }
                    let Some(ref f) = el.frame else {
                        return Err(step_err(ToolOutput::json(err_json(
                            "target_identity_unverified",
                            "Target lacks bounding frame",
                        ))));
                    };
                    let exp = expected_target_for_element(&el);
                    (f.x + f.w * 0.5, f.y + f.h * 0.5, !is_native, exp)
                } else {
                    let (px, py, _) =
                        resolve_batch_point(step, &self.elements_by_ref, &self.marks_by_id)
                            .map_err(step_err)?;
                    let exp = expected_target_from_args(step, &self.elements_by_ref, &self.marks_by_id);
                    (px, py, true, exp)
                };
                let click_first = step
                    .get("click_first")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(default_click_first);
                let eff_exp = if mode == "replace" || !click_first {
                    ExpectedPointTarget::Editable
                } else {
                    exp
                };
                (
                    ActionRequest::TypePoint {
                        bundle_id: cur_app.clone(),
                        x,
                        y,
                        text: text.into(),
                        mode: mode.into(),
                        click_first,
                        key_delay_us: self.config.timing.key_delay_us,
                    },
                    vec![(x, y)],
                    eff_exp,
                )
            }
            "set_value" => {
                let Some(text) = step.get("text").and_then(|v| v.as_str()) else {
                    return Err(step_err(ToolOutput::json(err_json(
                        "invalid_argument",
                        &format!("Step {idx} missing text"),
                    ))));
                };
                validate_input_text(text, true).map_err(step_err)?;
                let (resolved_app, _, x, y, _) = self
                    .resolve_editable_target("set_value", step, Some(cur_app))
                    .map_err(step_err)?;
                *cur_app = resolved_app;
                *step_filtered = self.snapshot_filtered;
                (
                    ActionRequest::SetValue {
                        bundle_id: cur_app.clone(),
                        x,
                        y,
                        text: text.into(),
                        key_delay_us: self.config.timing.key_delay_us,
                    },
                    vec![(x, y)],
                    ExpectedPointTarget::Editable,
                )
            }
            "select_text" => {
                let (start, end) = parse_select_range(step).map_err(step_err)?;
                let (resolved_app, _, x, y, _) = self
                    .resolve_editable_target("select_text", step, Some(cur_app))
                    .map_err(step_err)?;
                *cur_app = resolved_app;
                *step_filtered = self.snapshot_filtered;
                (
                    ActionRequest::SelectText {
                        bundle_id: cur_app.clone(),
                        x,
                        y,
                        start,
                        end,
                    },
                    vec![(x, y)],
                    ExpectedPointTarget::Editable,
                )
            }
            "press_key" => {
                *navigated = true;
                let Some(key) = step.get("key").and_then(|v| v.as_str()) else {
                    return Err(step_err(ToolOutput::json(err_json(
                        "invalid_argument",
                        &format!("Step {idx} missing key"),
                    ))));
                };
                let mods: Vec<String> = step
                    .get("modifiers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_ascii_lowercase()))
                            .collect()
                    })
                    .unwrap_or_default();
                if !valid_key_request(key, &mods) {
                    return Err(step_err(ToolOutput::json(err_json(
                        "invalid_argument",
                        "Unsupported key or blocked paste shortcut",
                    ))));
                }
                (
                    ActionRequest::PressKey {
                        bundle_id: cur_app.clone(),
                        key: key.into(),
                        modifiers: mods,
                    },
                    Vec::new(),
                    ExpectedPointTarget::Any,
                )
            }
            "scroll" => {
                *navigated = true;
                let target_spec = parse_semantic_target(step).map_err(step_err)?;
                let (dir, amount, target_pt) = if let Some(spec) = target_spec {
                    let (dir, amt) = parse_scroll_dir_amount(step).map_err(step_err)?;
                    let (resolved_app, el) = self
                        .resolve_semantic_target(Some(cur_app), step, &spec)
                        .map_err(step_err)?;
                    *cur_app = resolved_app;
                    *step_filtered = self.snapshot_filtered;
                    let Some(ref f) = el.frame else {
                        return Err(step_err(ToolOutput::json(err_json(
                            "target_identity_unverified",
                            "Element lacks coordinates",
                        ))));
                    };
                    (dir, amt, Some((f.x + f.w * 0.5, f.y + f.h * 0.5)))
                } else {
                    let (dir, amt, pt, _) = resolve_scroll_args(
                        step,
                        &self.elements_by_ref,
                        &self.marks_by_id,
                        *step_filtered,
                    )
                    .map_err(step_err)?;
                    (dir, amt, pt)
                };
                let (x, y, pts) = match target_pt {
                    Some((px, py)) => (Some(px), Some(py), vec![(px, py)]),
                    None => (None, None, Vec::new()),
                };
                (
                    ActionRequest::Scroll {
                        bundle_id: cur_app.clone(),
                        direction: dir,
                        amount,
                        x,
                        y,
                    },
                    pts,
                    ExpectedPointTarget::Any,
                )
            }
            _ => {
                return Err(step_err(ToolOutput::json(err_json(
                    "invalid_argument",
                    &format!("Unsupported batch step action: {act}"),
                ))));
            }
        };

        if matches!(act, "click" | "double_click" | "right_click") && self.settings.confirm_destructive {
            if let ExpectedPointTarget::Element { ref label, .. } = expected_target {
                if is_destructive_label(label) {
                    return Err(step_err(ToolOutput::json(err_json(
                        "confirmation_required",
                        &format!(
                            "Destructive action '{label}' requires confirmation via computer action='click' + action='confirm'"
                        ),
                    ))));
                }
            }
        }

        self.verify_points_with_identity(cur_app, &safety_pts, &expected_target)
            .map_err(step_err)?;
        let cached_ax = if idx == 0
            && initial_window_unscoped
            && initial_app.as_deref() == Some(cur_app.as_str())
        {
            initial_ax
        } else {
            None
        };
        let recheck_fast = self.driver.supports_fast_point_verify();
        let (method, telemetry) = self
            .execute_verified_request(
                cur_app,
                recheck_fast,
                cached_ax,
                &safety_pts,
                &req,
                &verify_opts,
            )
            .map_err(step_err)?;
        let step_failed_change =
            verify_opts.require_change && telemetry.is_some_and(|t| !t.verified_change);
        let mut item = vec![
            ("step", JsonValue::Number(idx as f64)),
            ("action", JsonValue::String(act.into())),
            ("method", JsonValue::String(method)),
            ("ok", JsonValue::Bool(!step_failed_change)),
        ];
        if let Some(t) = telemetry {
            append_verify_fields(&mut item, &t);
        }
        let item_val = JsonValue::object(item);
        if step_failed_change {
            let err = ToolOutput::json(err_json(
                "no_change_detected",
                &format!("Batch step {idx} ({act}) produced no detectable AX or visual change"),
            ));
            return Err((err, Some(item_val)));
        }
        Ok(item_val)
    }

    fn wait_for_condition(
        &mut self,
        sub_args: &JsonValue,
        default_app: Option<&str>,
    ) -> Result<JsonValue, ToolOutput> {
        let raw_app = match sub_args.get("app") {
            None | Some(JsonValue::Null) => match default_app {
                Some(a) => a,
                None => {
                    return Err(ToolOutput::json(err_json(
                        "invalid_argument",
                        "Missing arguments.app",
                    )))
                }
            },
            Some(v) => v.as_str().ok_or_else(|| {
                ToolOutput::json(err_json("invalid_argument", "'app' must be a string"))
            })?,
        };
        let Some(target) = self.resolve_target(raw_app) else {
            return Err(ToolOutput::json(err_json("invalid_app", "Target is invalid, protected, or not permitted")));
        };
        let query = match sub_args.get("query") {
            None | Some(JsonValue::Null) => match sub_args.get("text") {
                None | Some(JsonValue::Null) => {
                    return Err(ToolOutput::json(err_json(
                        "invalid_argument",
                        "Missing arguments.query",
                    )))
                }
                Some(v) => v.as_str().ok_or_else(|| {
                    ToolOutput::json(err_json("invalid_argument", "Invalid arguments.query"))
                })?,
            },
            Some(v) => v.as_str().ok_or_else(|| {
                ToolOutput::json(err_json("invalid_argument", "Invalid arguments.query"))
            })?,
        };
        if query.is_empty() || query.len() > 120 || query.chars().any(|c| c.is_control()) {
            return Err(ToolOutput::json(err_json("invalid_argument", "Invalid arguments.query")));
        }
        let want_role = match sub_args.get("role") {
            None | Some(JsonValue::Null) => None,
            Some(v) => match v.as_str() {
                Some(r) if !r.trim().is_empty() && !r.chars().any(|c| c.is_control()) => {
                    Some(r.trim().to_string())
                }
                _ => {
                    return Err(ToolOutput::json(err_json(
                        "invalid_argument",
                        "Invalid arguments.role",
                    )))
                }
            },
        };
        let timeout_ms = sub_args
            .get("timeout_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(15_000)
            .clamp(100, 60_000) as u64;
        let poll_ms = sub_args
            .get("poll_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(400)
            .clamp(50, 2_000) as u64;
        let absent = sub_args
            .get("absent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let use_ocr = sub_args
            .get("ocr")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let window = parse_optional_window(sub_args)?;

        let start = Instant::now();
        let deadline = Duration::from_millis(timeout_ms);
        let q_lower = query.to_ascii_lowercase();

        loop {
            if self.stop_file.exists() {
                return Err(ToolOutput::json(err_json("panic_stop", "STOP file present")));
            }
            self.driver.tick();
            let snap = match self.driver.inspect_app(&target, window, 150, 12) {
                Ok(s) => s,
                Err(e) => {
                    let transient = matches!(e.code.as_str(), "app_not_running" | "window_not_found");
                    let elapsed_now = start.elapsed();
                    if transient && elapsed_now < deadline {
                        let rem = deadline - elapsed_now;
                        thread::sleep(rem.min(Duration::from_millis(poll_ms)));
                        continue;
                    }
                    return Err(ToolOutput::json(err_json(&e.code, &e.message)));
                }
            };
            if !self.check_frontmost_permitted(&snap.frontmost_bundle_id) {
                return Err(ToolOutput::json(err_json("invalid_app", "Frontmost application is protected")));
            }
            if let Some(reason) = snapshot_redaction_reason(&snap) {
                return Err(ToolOutput::json(err_json("unsafe_or_incomplete_tree", &reason)));
            }
            let full_outline = snap.render_outline(None);
            if contains_secure_marker(&full_outline) {
                return Err(ToolOutput::json(err_json("unsafe_or_incomplete_tree", "sensitive_text_marker")));
            }

            let ax_found = if let Some(ref r) = want_role {
                snap.elements.iter().any(|el| {
                    role_matches(&el.role, r)
                        && (el.label.to_ascii_lowercase().contains(&q_lower)
                            || el
                                .value
                                .as_deref()
                                .is_some_and(|v| v.to_ascii_lowercase().contains(&q_lower)))
                })
            } else {
                full_outline.to_ascii_lowercase().contains(&q_lower)
            };
            let mut ocr_match_text: Option<String> = None;
            if use_ocr && !ax_found && want_role.is_none() {
                if let Ok(cap) = self.driver.capture_and_ocr(
                    &target,
                    window,
                    OcrMode::Fast,
                    false,
                    &ImageEncodeOptions::default(),
                ) {
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
                self.record_snapshot_meta(&snap, window, 150, 12, !absent && ax_found);
                return Ok(JsonValue::object(vec![
                    ("ok", JsonValue::Bool(true)),
                    ("matched", JsonValue::Bool(true)),
                    ("elapsed_ms", JsonValue::Number(elapsed)),
                    ("source", JsonValue::String(source.into())),
                    ("content", JsonValue::String(content)),
                ]));
            }

            let elapsed_now = start.elapsed();
            if elapsed_now >= deadline {
                return Ok(JsonValue::object(vec![
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

    fn handle_wait_for(&mut self, sub_args: &JsonValue) -> ToolOutput {
        if let Some(blocked) = self.preflight_gate("wait_for", false) {
            return blocked;
        }
        if self.audit.record("wait_for", "allow", "requested").is_err() {
            return ToolOutput::json(err_json("audit_unavailable", "Audit log write failed"));
        }
        match self.wait_for_condition(sub_args, None) {
            Ok(v) => ToolOutput::json(v),
            Err(e) => e,
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

        let initial_snap = match self.driver.inspect_app(&bundle, None, 250, 14) {
            Ok(s) => s,
            Err(e) => return ToolOutput::json(err_json(&e.code, &e.message)),
        };
        if let Err(redaction) = self.check_ax_snapshot_safety(&initial_snap) {
            return redaction_to_error(redaction);
        }
        if let Err(reason) = validate_recipe_preconditions(&recipe, &initial_snap) {
            self.invalidate_snapshot();
            return ToolOutput::json(err_json("precondition_failed", &reason));
        }

        let step_args = args.get("step_args").and_then(|v| v.as_array());
        let mut results = Vec::new();
        let mut cached_first_snap = Some(initial_snap);
        for (idx, step) in recipe.steps.iter().enumerate() {
            if self.stop_file.exists() {
                self.invalidate_snapshot();
                return replay_step_failure("panic_stop", "STOP file present", idx, None, results);
            }
            let override_arg = step_args.and_then(|arr| arr.get(idx));
            let act = step.action.as_str();
            match act {
                "click" | "double_click" | "right_click" | "type" | "set_value" => {
                    let snap = if let Some(s) = cached_first_snap.take() {
                        s
                    } else {
                        match self.driver.inspect_app(&bundle, None, 250, 14) {
                            Ok(s) => s,
                            Err(e) => {
                                self.invalidate_snapshot();
                                return replay_step_failure(&e.code, &e.message, idx, None, results);
                            }
                        }
                    };
                    if let Err(redaction) = self.check_ax_snapshot_safety(&snap) {
                        self.invalidate_snapshot();
                        return attach_step_failure(
                            redaction_to_error(redaction),
                            "completed_steps",
                            idx,
                            results,
                        );
                    }
                    let Some(el) = find_recipe_element(&snap.elements, step) else {
                        self.invalidate_snapshot();
                        return replay_step_failure(
                            "replay_target_missing",
                            "Matching replay target element not found",
                            idx,
                            Some(&step.target_label),
                            results,
                        );
                    };
                    if matches!(act, "click" | "double_click" | "right_click")
                        && self.settings.confirm_destructive
                        && (is_destructive_label(&el.label) || is_destructive_label(&step.target_label))
                    {
                        self.invalidate_snapshot();
                        return replay_step_failure(
                            "confirmation_required",
                            &format!(
                                "Destructive replay action '{}' requires interactive confirmation",
                                step.target_label
                            ),
                            idx,
                            None,
                            results,
                        );
                    }
                    let Some(ref f) = el.frame else {
                        self.invalidate_snapshot();
                        return replay_step_failure(
                            "target_identity_unverified",
                            "Matched element lacks frame",
                            idx,
                            None,
                            results,
                        );
                    };
                    let (cx, cy) = (f.x + f.w * 0.5, f.y + f.h * 0.5);
                    let req = if act == "set_value" {
                        if !is_editable_role(&el.role) {
                            self.invalidate_snapshot();
                            return replay_step_failure(
                                "target_identity_unverified",
                                "Replay set_value target is not editable",
                                idx,
                                None,
                                results,
                            );
                        }
                        let Some(text) = override_arg.and_then(|v| v.get("text")).and_then(|v| v.as_str()) else {
                            self.invalidate_snapshot();
                            return replay_step_failure(
                                "invalid_argument",
                                &format!("Replay step {idx} requires step_args[{idx}].text"),
                                idx,
                                None,
                                results,
                            );
                        };
                        if let Err(e) = validate_input_text(text, true) {
                            self.invalidate_snapshot();
                            return batch_failure_or_replay(e, idx, results);
                        }
                        ActionRequest::SetValue {
                            bundle_id: bundle.clone(),
                            x: cx,
                            y: cy,
                            text: text.into(),
                            key_delay_us: self.config.timing.key_delay_us,
                        }
                    } else if act == "type" {
                        let Some(text) = override_arg.and_then(|v| v.get("text")).and_then(|v| v.as_str()) else {
                            self.invalidate_snapshot();
                            return replay_step_failure(
                                "invalid_argument",
                                &format!("Replay step {idx} requires step_args[{idx}].text"),
                                idx,
                                None,
                                results,
                            );
                        };
                        if let Err(e) = validate_input_text(text, false) {
                            self.invalidate_snapshot();
                            return batch_failure_or_replay(e, idx, results);
                        }
                        let is_native_text = is_editable_role(&el.role);
                        ActionRequest::TypePoint {
                            bundle_id: bundle.clone(),
                            x: cx,
                            y: cy,
                            text: text.into(),
                            mode: "insert".into(),
                            click_first: !is_native_text,
                            key_delay_us: self.config.timing.key_delay_us,
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
                    let mut opts = ActionVerifyOptions::from_timing(&self.config.timing);
                    if step.verify_change {
                        opts.verify_change = true;
                        opts.require_change = true;
                    }
                    let safety_pts = [(cx, cy)];
                    match self.execute_verified_request(
                        &bundle,
                        self.driver.supports_fast_point_verify(),
                        Some(&snap.comparable_text()),
                        &safety_pts,
                        &req,
                        &opts,
                    ) {
                        Ok((method, telemetry)) => {
                            let no_change =
                                opts.require_change && telemetry.is_some_and(|t| !t.verified_change);
                            let mut item = vec![
                                ("step", JsonValue::Number(idx as f64)),
                                ("action", JsonValue::String(act.into())),
                                ("method", JsonValue::String(method)),
                            ];
                            if let Some(t) = telemetry {
                                append_verify_fields(&mut item, &t);
                            }
                            results.push(JsonValue::object(item));
                            if no_change {
                                self.invalidate_snapshot();
                                return replay_step_failure(
                                    "no_change_detected",
                                    &format!("Replay step {idx} ({act}) produced no detectable change"),
                                    idx,
                                    None,
                                    results,
                                );
                            }
                        }
                        Err(e) => {
                            self.invalidate_snapshot();
                            return batch_failure_or_replay(e, idx, results);
                        }
                    }
                }
                "press_key" => {
                    let _ = cached_first_snap.take();
                    let Some(key) = override_arg.and_then(|v| v.get("key")).and_then(|v| v.as_str()) else {
                        self.invalidate_snapshot();
                        return replay_step_failure(
                            "invalid_argument",
                            &format!("Replay step {idx} requires step_args[{idx}].key"),
                            idx,
                            None,
                            results,
                        );
                    };
                    let mods: Vec<String> = override_arg
                        .and_then(|v| v.get("modifiers"))
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_ascii_lowercase())).collect())
                        .unwrap_or_default();
                    if !valid_key_request(key, &mods) {
                        self.invalidate_snapshot();
                        return replay_step_failure("invalid_argument", "Unsupported replay key", idx, None, results);
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
                        Err(e) => {
                            self.invalidate_snapshot();
                            return replay_step_failure(&e.code, &e.message, idx, None, results);
                        }
                    }
                }
                _ => {
                    self.invalidate_snapshot();
                    return replay_step_failure(
                        "invalid_argument",
                        &format!("Unsupported replay action: {act}"),
                        idx,
                        None,
                        results,
                    );
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

fn resolve_image_options(args: &JsonValue, config: &AppConfig) -> Result<ImageEncodeOptions, ToolOutput> {
    let format = match args.get("format") {
        None | Some(JsonValue::Null) => config.image.format,
        Some(v) => match v.as_str() {
            Some(s) => match s.trim().to_ascii_lowercase().as_str() {
                "jpeg" | "jpg" => ImageFormat::Jpeg,
                "png" => ImageFormat::Png,
                _ => {
                    return Err(ToolOutput::json(err_json(
                        "invalid_argument",
                        "'format' must be 'jpeg' or 'png'",
                    )))
                }
            },
            None => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'format' must be a string ('jpeg' or 'png')",
                )))
            }
        },
    };
    let quality = match args.get("quality") {
        None | Some(JsonValue::Null) => config.image.quality,
        Some(v) => match v.as_i64() {
            Some(n) if (1..=100).contains(&n) => n as u8,
            _ => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'quality' must be an integer between 1 and 100",
                )))
            }
        },
    };
    let max_width = match args.get("max_width") {
        None | Some(JsonValue::Null) => config.image.max_width,
        Some(v) => match v.as_i64() {
            Some(n) if (640..=2560).contains(&n) => n as usize,
            _ => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'max_width' must be an integer between 640 and 2560",
                )))
            }
        },
    };
    Ok(ImageEncodeOptions {
        format,
        quality,
        max_width,
    })
}

fn scaled_dimensions(src_w: usize, src_h: usize, max_width: usize) -> (usize, usize) {
    if src_w == 0 || src_h == 0 || max_width == 0 || src_w <= max_width {
        return (src_w, src_h);
    }
    let out_w = max_width;
    let out_h = ((src_h as f64) * (out_w as f64) / (src_w as f64))
        .round()
        .max(1.0) as usize;
    (out_w, out_h)
}

fn ensure_encoded_image_b64(
    raw_b64: String,
    src_w: usize,
    src_h: usize,
    options: &ImageEncodeOptions,
) -> Result<(String, usize, usize), ToolOutput> {
    if options.format == ImageFormat::Jpeg && !raw_b64.starts_with("/9j/") {
        if let Ok(bytes) = base64_decode(&raw_b64) {
            if let Ok((w, h, rgba)) = decode_png_rgba(&bytes) {
                let (dw, dh, scaled) = if options.max_width > 0 && w > options.max_width {
                    downscale_rgba(&rgba, w, h, options.max_width)
                } else {
                    (w, h, rgba)
                };
                return Ok((
                    encode_rgba_image_b64(dw, dh, &scaled, ImageFormat::Jpeg, options.quality),
                    dw,
                    dh,
                ));
            }
        }
    }
    let (out_w, out_h) = scaled_dimensions(src_w, src_h, options.max_width);
    Ok((raw_b64, out_w, out_h))
}

fn base64_decoded_len(b64: &str) -> usize {
    let non_ws = b64.bytes().filter(|b| !b.is_ascii_whitespace()).count();
    if non_ws < 4 {
        return 0;
    }
    let pad = b64
        .bytes()
        .rev()
        .filter(|b| !b.is_ascii_whitespace())
        .take(2)
        .filter(|&b| b == b'=')
        .count();
    (non_ws / 4).saturating_mul(3).saturating_sub(pad)
}

fn rects_approx_equal(a: Option<&Rect>, b: Option<&Rect>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(ra), Some(rb)) => {
            (ra.x - rb.x).abs() <= 0.5
                && (ra.y - rb.y).abs() <= 0.5
                && (ra.w - rb.w).abs() <= 0.5
                && (ra.h - rb.h).abs() <= 0.5
        }
        _ => false,
    }
}

fn validate_input_text(text: &str, allow_empty: bool) -> Result<(), ToolOutput> {
    let max_len = 2000;
    if (!allow_empty && text.is_empty()) || text.chars().count() > max_len {
        return Err(ToolOutput::json(err_json(
            "invalid_argument",
            "Text length out of allowed range",
        )));
    }
    if !text.chars().all(|c| !c.is_control() || matches!(c, '\n' | '\t')) {
        return Err(ToolOutput::json(err_json(
            "invalid_argument",
            "Control characters are not allowed",
        )));
    }
    Ok(())
}

fn parse_select_range(args: &JsonValue) -> Result<(Option<usize>, Option<usize>), ToolOutput> {
    let start = match args.get("start") {
        None | Some(JsonValue::Null) => None,
        Some(v) => match v.as_i64() {
            Some(n) if n >= 0 => Some(n as usize),
            _ => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'start' must be a non-negative integer",
                )))
            }
        },
    };
    let end = match args.get("end") {
        None | Some(JsonValue::Null) => None,
        Some(v) => match v.as_i64() {
            Some(n) if n >= 0 => Some(n as usize),
            _ => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'end' must be a non-negative integer",
                )))
            }
        },
    };
    if let (Some(s), Some(e)) = (start, end) {
        if s > e {
            return Err(ToolOutput::json(err_json(
                "invalid_argument",
                "'start' must be <= 'end'",
            )));
        }
    }
    Ok((start, end))
}

fn parse_optional_window(args: &JsonValue) -> Result<Option<i32>, ToolOutput> {
    match args.get("window") {
        None | Some(JsonValue::Null) => Ok(None),
        Some(v) => match v.as_i64() {
            Some(n) if n >= 0 => Ok(Some(n as i32)),
            _ => Err(ToolOutput::json(err_json(
                "invalid_argument",
                "'window' must be a non-negative integer",
            ))),
        },
    }
}

fn parse_optional_query(args: &JsonValue) -> Result<Option<&str>, ToolOutput> {
    match args.get("query") {
        None | Some(JsonValue::Null) => Ok(None),
        Some(v) => match v.as_str() {
            Some(q) if !q.is_empty() && q.len() <= 120 && q.chars().all(|c| !c.is_control()) => {
                Ok(Some(q))
            }
            _ => Err(ToolOutput::json(err_json(
                "invalid_argument",
                "Invalid 'query' string",
            ))),
        },
    }
}

fn parse_optional_bool(args: &JsonValue, key: &str, default: bool) -> Result<bool, ToolOutput> {
    Ok(parse_optional_bool_opt(args, key)?.unwrap_or(default))
}

fn parse_optional_bool_opt(args: &JsonValue, key: &str) -> Result<Option<bool>, ToolOutput> {
    match args.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(v) => match v.as_bool() {
            Some(b) => Ok(Some(b)),
            None => Err(ToolOutput::json(err_json(
                "invalid_argument",
                &format!("'{key}' must be a boolean"),
            ))),
        },
    }
}

fn parse_verify_mode(args: &JsonValue) -> Result<VerifyMode, ToolOutput> {
    match args.get("verify_mode") {
        None | Some(JsonValue::Null) => Ok(VerifyMode::Auto),
        Some(v) => match v.as_str() {
            Some(s) => match s.trim().to_ascii_lowercase().as_str() {
                "auto" => Ok(VerifyMode::Auto),
                "ax" => Ok(VerifyMode::Ax),
                "visual" => Ok(VerifyMode::Visual),
                _ => Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'verify_mode' must be 'auto', 'ax', or 'visual'",
                ))),
            },
            None => Err(ToolOutput::json(err_json(
                "invalid_argument",
                "'verify_mode' must be a string ('auto', 'ax', or 'visual')",
            ))),
        },
    }
}

fn parse_return_state_and_bounds(args: &JsonValue) -> Result<(ReturnStateMode, Option<bool>), ToolOutput> {
    let return_state = match args.get("return_state") {
        None | Some(JsonValue::Null) => ReturnStateMode::None,
        Some(v) => match v.as_str() {
            Some(s) => match s.trim().to_ascii_lowercase().as_str() {
                "none" => ReturnStateMode::None,
                "diff" => ReturnStateMode::Diff,
                "full" => ReturnStateMode::Full,
                _ => {
                    return Err(ToolOutput::json(err_json(
                        "invalid_argument",
                        "'return_state' must be 'none', 'diff', or 'full'",
                    )))
                }
            },
            None => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'return_state' must be a string ('none', 'diff', or 'full')",
                )))
            }
        },
    };
    let include_bounds = parse_optional_bool_opt(args, "include_bounds")?;
    Ok((return_state, include_bounds))
}

fn has_non_null(args: &JsonValue, key: &str) -> bool {
    matches!(args.get(key), Some(v) if !v.is_null())
}

fn parse_semantic_target(args: &JsonValue) -> Result<Option<SemanticTargetSpec>, ToolOutput> {
    let Some(val) = args.get("target") else {
        return Ok(None);
    };
    if matches!(val, JsonValue::Null) {
        return Ok(None);
    }
    if has_non_null(args, "ref")
        || has_non_null(args, "mark")
        || has_non_null(args, "x")
        || has_non_null(args, "y")
    {
        return Err(ToolOutput::json(err_json(
            "invalid_argument",
            "Provide only one of target, ref, mark, or x/y",
        )));
    }
    let Some(obj) = val.as_object() else {
        return Err(ToolOutput::json(err_json(
            "invalid_argument",
            "'target' must be an object with 'query'",
        )));
    };
    let Some(raw_query) = obj.get("query").and_then(|v| v.as_str()) else {
        return Err(ToolOutput::json(err_json(
            "invalid_argument",
            "'target.query' must be a non-empty string",
        )));
    };
    let query = raw_query.trim();
    if query.is_empty() || query.chars().count() > 200 || query.chars().any(|c| c.is_control()) {
        return Err(ToolOutput::json(err_json(
            "invalid_argument",
            "Invalid 'target.query'",
        )));
    }
    let role = match obj.get("role") {
        None | Some(JsonValue::Null) => None,
        Some(v) => match v.as_str() {
            Some(r) => {
                let t = r.trim();
                if t.is_empty() || t.chars().any(|c| c.is_control()) {
                    return Err(ToolOutput::json(err_json(
                        "invalid_argument",
                        "Invalid 'target.role'",
                    )));
                }
                Some(t.to_string())
            }
            None => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'target.role' must be a string",
                )))
            }
        },
    };
    let fresh = match obj.get("fresh") {
        None | Some(JsonValue::Null) => false,
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'target.fresh' must be a boolean",
                )))
            }
        },
    };
    Ok(Some(SemanticTargetSpec {
        query: query.to_string(),
        role,
        fresh,
    }))
}

fn role_matches(el_role: &str, wanted_role: &str) -> bool {
    if el_role.eq_ignore_ascii_case(wanted_role) {
        return true;
    }
    let stripped = wanted_role
        .strip_prefix("AX")
        .or_else(|| wanted_role.strip_prefix("ax"))
        .unwrap_or(wanted_role);
    if el_role.eq_ignore_ascii_case(stripped) {
        return true;
    }
    let norm = normalize_role(stripped);
    norm != "Element" && el_role.eq_ignore_ascii_case(norm)
}

#[derive(Default)]
struct TierMatches<'a> {
    enabled: Vec<&'a UiElement>,
    has_disabled: bool,
}

fn match_semantic_in_elements(
    elements: &HashMap<String, UiElement>,
    spec: &SemanticTargetSpec,
) -> Result<UiElement, SemanticMatchError> {
    let q_lower = spec.query.trim().to_ascii_lowercase();
    let mut tiers = [
        TierMatches::default(),
        TierMatches::default(),
        TierMatches::default(),
        TierMatches::default(),
    ];

    for el in elements.values() {
        if el.frame.is_none() {
            continue;
        }
        if let Some(ref want_role) = spec.role {
            if !role_matches(&el.role, want_role) {
                continue;
            }
        }
        let lbl = el.label.trim().to_ascii_lowercase();
        let val = el
            .value
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let lbl_exact = lbl == q_lower;
        let lbl_part = !lbl_exact && lbl.contains(&q_lower);
        let val_exact = !val.is_empty() && val == q_lower;
        let val_part = !val.is_empty() && !val_exact && val.contains(&q_lower);
        if !(lbl_exact || lbl_part || val_exact || val_part) {
            continue;
        }
        let tier_idx = if lbl_exact {
            0
        } else if lbl_part {
            1
        } else if val_exact {
            2
        } else {
            3
        };
        if el.enabled == Some(false) {
            tiers[tier_idx].has_disabled = true;
        } else {
            tiers[tier_idx].enabled.push(el);
        }
    }

    for tier in tiers {
        if tier.enabled.len() == 1 {
            return Ok(tier.enabled[0].clone());
        }
        if tier.enabled.len() > 1 {
            return Err(SemanticMatchError::Ambiguous);
        }
        if tier.has_disabled {
            return Err(SemanticMatchError::Disabled);
        }
    }
    Err(SemanticMatchError::NotFound)
}

fn parse_verify_options(args: &JsonValue, timing: &TimingConfig) -> Result<ActionVerifyOptions, ToolOutput> {
    let require_change = match args.get("require_change") {
        None | Some(JsonValue::Null) => false,
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'require_change' must be a boolean",
                )))
            }
        },
    };
    let max_retries = match args.get("max_retries") {
        None | Some(JsonValue::Null) => 0u8,
        Some(v) => match v.as_i64() {
            Some(n) if (0..=3).contains(&n) => n as u8,
            _ => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'max_retries' must be an integer between 0 and 3",
                )))
            }
        },
    };
    let explicit_verify = match args.get("verify_change") {
        None | Some(JsonValue::Null) => false,
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'verify_change' must be a boolean",
                )))
            }
        },
    };
    let settle_ms = match args.get("settle_ms") {
        None | Some(JsonValue::Null) => timing.settle_ms,
        Some(v) => match v.as_i64() {
            Some(n) if (10..=3000).contains(&n) => n as u64,
            _ => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'settle_ms' must be an integer between 10 and 3000",
                )))
            }
        },
    };
    let poll_interval_ms = match args.get("poll_interval_ms") {
        None | Some(JsonValue::Null) => timing.poll_interval_ms,
        Some(v) => match v.as_i64() {
            Some(n) if (10..=500).contains(&n) => n as u64,
            _ => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'poll_interval_ms' must be an integer between 10 and 500",
                )))
            }
        },
    };
    let retry_delay_ms = match args.get("retry_delay_ms") {
        None | Some(JsonValue::Null) => 250,
        Some(v) => match v.as_i64() {
            Some(n) if (10..=3000).contains(&n) => n as u64,
            _ => {
                return Err(ToolOutput::json(err_json(
                    "invalid_argument",
                    "'retry_delay_ms' must be an integer between 10 and 3000",
                )))
            }
        },
    };
    let keep_snapshot = parse_optional_bool_opt(args, "keep_snapshot")?;
    let verify_mode = parse_verify_mode(args)?;
    let (return_state, include_bounds) = parse_return_state_and_bounds(args)?;
    Ok(ActionVerifyOptions {
        verify_change: explicit_verify || max_retries > 0 || require_change,
        verify_mode,
        return_state,
        include_bounds,
        max_retries,
        settle_ms,
        poll_interval_ms,
        retry_delay_ms,
        require_change,
        keep_snapshot,
    })
}

fn append_verify_fields(fields: &mut Vec<(&'static str, JsonValue)>, t: &VerifyTelemetry) {
    fields.push(("verified_change", JsonValue::Bool(t.verified_change)));
    fields.push(("ax_changed", JsonValue::Bool(t.ax_changed)));
    fields.push(("visual_changed", JsonValue::Bool(t.visual_changed)));
    fields.push(("attempts", JsonValue::Number(t.attempts as f64)));
    fields.push(("retried", JsonValue::Number(t.retried as f64)));
}

fn decode_hex_bytes(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    let hex_val = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let hi = hex_val(chunk[0])?;
        let lo = hex_val(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn visual_digest_changed(before: &str, after: &str) -> bool {
    if before.len() != after.len() {
        return true;
    }
    if before.len() >= 32 {
        if let (Some(a), Some(b)) = (decode_hex_bytes(before), decode_hex_bytes(after)) {
            let mut changed_blocks = 0usize;
            let mut sum_abs = 0u64;
            for (&x, &y) in a.iter().zip(b.iter()) {
                let d = x.abs_diff(y);
                sum_abs += d as u64;
                if d >= 10 {
                    changed_blocks += 1;
                }
            }
            let mean_abs = sum_abs as f64 / a.len() as f64;
            return changed_blocks >= 1 || mean_abs >= 1.5;
        }
    }
    before != after
}

enum ExpectedPointTarget {
    Any,
    Editable,
    Element { role: String, label: String },
}

fn expected_target_for_element(el: &UiElement) -> ExpectedPointTarget {
    if is_editable_role(&el.role) {
        ExpectedPointTarget::Editable
    } else {
        ExpectedPointTarget::Element {
            role: el.role.clone(),
            label: el.label.clone(),
        }
    }
}

fn expected_target_for_action(
    action: &GatewayAction,
    elements: &HashMap<String, UiElement>,
    marks: &HashMap<String, VisualMark>,
) -> ExpectedPointTarget {
    match action {
        GatewayAction::Std(PendingAction::ClickRef { ref_id, .. }) => elements
            .get(ref_id)
            .map(expected_target_for_element)
            .unwrap_or(ExpectedPointTarget::Any),
        GatewayAction::Std(PendingAction::VisualClick {
            mark_id: Some(mid), ..
        }) => marks
            .get(mid)
            .filter(|m| m.ax_ref.is_some())
            .map(|m| ExpectedPointTarget::Element {
                role: m.role.clone(),
                label: m.label.clone(),
            })
            .unwrap_or(ExpectedPointTarget::Any),
        GatewayAction::Std(PendingAction::TypeText {
            ref_id: Some(rid),
            click_first,
            ..
        }) => {
            if !click_first {
                ExpectedPointTarget::Editable
            } else {
                elements
                    .get(rid)
                    .map(expected_target_for_element)
                    .unwrap_or(ExpectedPointTarget::Any)
            }
        }
        GatewayAction::Std(PendingAction::TypeText {
            click_first: false, ..
        })
        | GatewayAction::SetValue { .. }
        | GatewayAction::SelectText { .. } => ExpectedPointTarget::Editable,
        _ => ExpectedPointTarget::Any,
    }
}

fn expected_target_from_args(
    args: &JsonValue,
    elements: &HashMap<String, UiElement>,
    marks: &HashMap<String, VisualMark>,
) -> ExpectedPointTarget {
    if let Some(mid) = args.get("mark").and_then(|v| v.as_str()) {
        return marks
            .get(mid)
            .filter(|m| m.ax_ref.is_some())
            .map(|m| ExpectedPointTarget::Element {
                role: m.role.clone(),
                label: m.label.clone(),
            })
            .unwrap_or(ExpectedPointTarget::Any);
    }
    if let Some(rid) = args.get("ref").and_then(|v| v.as_str()) {
        return elements
            .get(rid)
            .map(expected_target_for_element)
            .unwrap_or(ExpectedPointTarget::Any);
    }
    ExpectedPointTarget::Any
}

fn point_probe_matches_expected(
    probe: &crate::driver::PointSafetyInfo,
    expected: &ExpectedPointTarget,
) -> bool {
    match expected {
        ExpectedPointTarget::Any => true,
        ExpectedPointTarget::Editable => {
            let Some(raw_role) = probe.role.as_deref() else {
                return true;
            };
            let hit_role = normalize_role(raw_role);
            hit_role.is_empty()
                || is_editable_role(&hit_role)
                || matches!(hit_role, "Group" | "ScrollArea" | "WebArea" | "StaticText")
        }
        ExpectedPointTarget::Element { role, label } => {
            let exp_role = normalize_role(role);
            if is_editable_role(&exp_role) {
                return point_probe_matches_expected(probe, &ExpectedPointTarget::Editable);
            }
            let hit_role = probe.role.as_deref().map(normalize_role).unwrap_or("");
            let hit_label = probe.label.as_deref().map(str::trim).unwrap_or("");
            let exp_label = label.trim();

            if !hit_role.is_empty() && !exp_role.is_empty() && hit_role != exp_role {
                let compat_combo = matches!(
                    (exp_role, hit_role),
                    ("PopUpButton", "ComboBox") | ("ComboBox", "PopUpButton")
                );
                let compat_child = matches!(hit_role, "StaticText" | "Image" | "Group");
                if !compat_combo && !compat_child {
                    return false;
                }
            }
            if !exp_label.is_empty() && !hit_label.is_empty() {
                let a = exp_label.to_ascii_lowercase();
                let b = hit_label.to_ascii_lowercase();
                if !hit_role.is_empty() && !exp_role.is_empty() && hit_role != exp_role {
                    return a == b || a.contains(&b) || b.contains(&a);
                }
                return a == b;
            }
            true
        }
    }
}

fn resolve_point_from_maps(
    args: &JsonValue,
    elements: &HashMap<String, UiElement>,
    marks: &HashMap<String, VisualMark>,
    ref_key: &str,
    mark_key: &str,
    x_key: &str,
    y_key: &str,
    check_enabled: bool,
) -> Result<(f64, f64, bool, String), ToolOutput> {
    if let Some(mid) = args.get(mark_key).and_then(|v| v.as_str()) {
        let Some(mark) = marks.get(mid) else {
            return Err(ToolOutput::json(err_json("stale_ref", "Unknown or stale visual mark ID")));
        };
        return Ok((
            mark.center_x,
            mark.center_y,
            mark.prefer_ax,
            format!("mark {mid} ({} \"{}\")", mark.role, mark.label),
        ));
    }
    if let Some(rid) = args.get(ref_key).and_then(|v| v.as_str()) {
        let Some(el) = elements.get(rid) else {
            return Err(ToolOutput::json(err_json("stale_ref", "Unknown or stale element ref")));
        };
        if check_enabled && el.enabled == Some(false) {
            return Err(ToolOutput::json(err_json("not_implemented", "Element is disabled")));
        }
        let Some(ref f) = el.frame else {
            return Err(ToolOutput::json(err_json("target_identity_unverified", "Element lacks coordinates")));
        };
        let label = if el.label.is_empty() { el.role.clone() } else { el.label.clone() };
        return Ok((
            f.x + f.w * 0.5,
            f.y + f.h * 0.5,
            supports_ax_press(el),
            format!("{} '{}' ({rid})", el.role, label),
        ));
    }
    if let (Some(x), Some(y)) = (
        args.get(x_key).and_then(|v| v.as_f64()),
        args.get(y_key).and_then(|v| v.as_f64()),
    ) {
        if !x.is_finite() || !y.is_finite() || x.abs() > 100000.0 || y.abs() > 100000.0 {
            return Err(ToolOutput::json(err_json("invalid_argument", "Coordinate out of bounds")));
        }
        return Ok((x, y, false, format!("({x:.0}, {y:.0})")));
    }
    Err(ToolOutput::json(err_json("invalid_argument", "Missing target ref, mark, or coordinates")))
}

fn resolve_batch_point(
    step: &JsonValue,
    elements: &HashMap<String, UiElement>,
    marks: &HashMap<String, VisualMark>,
) -> Result<(f64, f64, bool), ToolOutput> {
    let (x, y, prefer_ax, _) =
        resolve_point_from_maps(step, elements, marks, "ref", "mark", "x", "y", true)?;
    Ok((x, y, prefer_ax))
}

fn parse_scroll_dir_amount(args: &JsonValue) -> Result<(String, i32), ToolOutput> {
    let Some(direction) = args.get("direction").and_then(|v| v.as_str()) else {
        return Err(ToolOutput::json(err_json("invalid_argument", "Missing arguments.direction")));
    };
    if !matches!(direction, "up" | "down" | "left" | "right") {
        return Err(ToolOutput::json(err_json("invalid_argument", "Invalid scroll direction")));
    }
    let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
    if !(1..=100).contains(&amount) {
        return Err(ToolOutput::json(err_json("invalid_argument", "Scroll amount must be 1..=100")));
    }
    Ok((direction.to_string(), amount as i32))
}

fn resolve_scroll_args(
    args: &JsonValue,
    elements: &HashMap<String, UiElement>,
    marks: &HashMap<String, VisualMark>,
    snapshot_filtered: bool,
) -> Result<(String, i32, Option<(f64, f64)>, String), ToolOutput> {
    let (direction, amount) = parse_scroll_dir_amount(args)?;
    let has_ref_or_mark = has_non_null(args, "mark") || has_non_null(args, "ref");
    let has_coords = has_non_null(args, "x") || has_non_null(args, "y");
    if snapshot_filtered && !has_ref_or_mark {
        return Err(ToolOutput::json(err_json("invalid_state", "scroll requires an unfiltered snapshot")));
    }
    if has_ref_or_mark || has_coords {
        let (x, y, _, desc) =
            resolve_point_from_maps(args, elements, marks, "ref", "mark", "x", "y", false)?;
        return Ok((direction, amount, Some((x, y)), desc));
    }
    Ok((direction, amount, None, String::new()))
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
    const ALLOWED_MODS: &[&str] = &["cmd", "ctrl", "alt", "shift", "super", "win", "meta"];
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

fn snapshot_page_url(snap: &AppSnapshot) -> Option<String> {
    let find_role_url = |role: &str| {
        snap.elements.iter().find_map(|el| {
            if el.role == role {
                el.url
                    .as_deref()
                    .map(str::trim)
                    .filter(|u| extract_url_host(u).is_some())
                    .map(str::to_string)
            } else {
                None
            }
        })
    };
    find_role_url("WebArea").or_else(|| find_role_url("Window"))
}

fn validate_recipe_preconditions(
    recipe: &crate::memory::WorkflowRecipe,
    snap: &AppSnapshot,
) -> Result<(), String> {
    if let Some(ref exp_win) = recipe.expected_window_contains {
        let needle = exp_win.trim().to_ascii_lowercase();
        let matches_win = snap
            .windows
            .iter()
            .any(|w| w.title.to_ascii_lowercase().contains(&needle));
        if !matches_win {
            return Err(format!(
                "Active window does not match expected_window_contains '{exp_win}'"
            ));
        }
    }
    if let Some(ref exp_dom) = recipe.expected_domain {
        let Some(url) = snapshot_page_url(snap) else {
            return Err(format!(
                "No active page URL exposed to verify expected_domain '{exp_dom}'"
            ));
        };
        let Some(host) = extract_url_host(&url) else {
            return Err(format!(
                "Active URL '{url}' lacks a valid host for expected_domain '{exp_dom}'"
            ));
        };
        if !domain_matches(&host, exp_dom) {
            return Err(format!(
                "Active domain '{host}' does not match expected_domain '{exp_dom}'"
            ));
        }
    }
    Ok(())
}

fn redaction_to_error(redaction: SnapshotRedaction) -> ToolOutput {
    match redaction {
        SnapshotRedaction::ProtectedFrontmost => {
            ToolOutput::json(err_json("invalid_app", "Frontmost application is protected"))
        }
        SnapshotRedaction::DomainBlocked(msg) => {
            ToolOutput::json(err_json("domain_blocked", &msg))
        }
        SnapshotRedaction::Reason(reason) => {
            ToolOutput::json(err_json("unsafe_or_incomplete_tree", &reason))
        }
    }
}

fn attach_step_failure(
    mut err: ToolOutput,
    count_key: &'static str,
    failed_step: usize,
    completed_results: Vec<JsonValue>,
) -> ToolOutput {
    if let JsonValue::Object(ref mut map) = err.payload {
        map.insert("failed_step".into(), JsonValue::Number(failed_step as f64));
        map.insert(count_key.into(), JsonValue::Number(completed_results.len() as f64));
        map.insert("results".into(), JsonValue::Array(completed_results));
    }
    err
}

fn replay_step_failure(
    code: &str,
    message: &str,
    failed_step: usize,
    target_label: Option<&str>,
    completed_results: Vec<JsonValue>,
) -> ToolOutput {
    let mut err = attach_step_failure(
        ToolOutput::json(err_json(code, message)),
        "completed_steps",
        failed_step,
        completed_results,
    );
    if let (Some(lbl), JsonValue::Object(ref mut map)) = (target_label, &mut err.payload) {
        map.insert("target_label".into(), JsonValue::String(lbl.into()));
    }
    err
}

fn batch_failure_or_replay(
    err: ToolOutput,
    failed_step: usize,
    completed_results: Vec<JsonValue>,
) -> ToolOutput {
    attach_step_failure(err, "completed_steps", failed_step, completed_results)
}

fn batch_failure_output(
    err: ToolOutput,
    failed_step: usize,
    completed_results: Vec<JsonValue>,
) -> ToolOutput {
    attach_step_failure(err, "executed", failed_step, completed_results)
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
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let r = ((x * 2) % 255) as u8;
            let g = ((y * 2) % 255) as u8;
            rgba.extend_from_slice(&[r, g, 180, 255]);
        }
    }
    crate::som_png::encode_rgba_png_b64(w, h, &rgba)
}

pub fn base64_encode(bytes: &[u8]) -> String {
    crate::som_png::base64_encode(bytes)
}

/// Deterministic mock driver used by Rust unit tests and `--mock` MCP smoke tests.
pub struct MockDriver {
    pub click_log: std::sync::Mutex<Vec<String>>,
    os: String,
    accessibility: bool,
    screen_recording: bool,
}

impl MockDriver {
    pub fn new() -> Self {
        Self::with_permissions("mock", true, true)
    }

    pub fn with_permissions(
        os: impl Into<String>,
        accessibility: bool,
        screen_recording: bool,
    ) -> Self {
        Self {
            click_log: std::sync::Mutex::new(Vec::new()),
            os: os.into(),
            accessibility,
            screen_recording,
        }
    }
}

impl OsDriver for MockDriver {
    fn doctor(&self) -> Result<DoctorStatus, crate::driver::DriverError> {
        Ok(DoctorStatus {
            os: self.os.clone(),
            accessibility: self.accessibility,
            screen_recording: self.screen_recording,
            note: "Mock driver active for deterministic verification".into(),
        })
    }

    fn onboard(
        &self,
        _force: bool,
        _auto_close: bool,
    ) -> Result<crate::driver::OnboardStatus, crate::driver::DriverError> {
        let completed = self.accessibility && self.screen_recording;
        let note = if self.os == "macos" {
            if completed {
                "Permissions already granted".into()
            } else {
                "Mock onboarding (UI suppressed in mock mode)".into()
            }
        } else if self.os == "mock" {
            "Mock driver onboarding (no GUI window spawned)".into()
        } else {
            "macOS permission onboarding is only required on macOS".into()
        };
        Ok(crate::driver::OnboardStatus {
            os: self.os.clone(),
            shown: false,
            already_granted: completed,
            accessibility: self.accessibility,
            screen_recording: self.screen_recording,
            completed,
            host_app: None,
            note,
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
                    url: None,
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
                    url: None,
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

    fn window_state(
        &self,
        bundle_id: &str,
    ) -> Result<WindowState, crate::driver::DriverError> {
        let front = if bundle_id == "desktop" {
            "com.apple.calculator"
        } else {
            bundle_id
        };
        Ok(WindowState {
            frontmost_bundle_id: front.into(),
            pid: 4242,
            windows_total: 1,
            active_window_frame: Some(Rect {
                x: 100.0,
                y: 100.0,
                w: 320.0,
                h: 440.0,
            }),
        })
    }

    fn capture_and_ocr(
        &self,
        bundle_id: &str,
        _window: Option<i32>,
        ocr_mode: OcrMode,
        _encode_image: bool,
        options: &ImageEncodeOptions,
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
        let mut rgba = Vec::with_capacity(64 * 64 * 4);
        for y in 0..64 {
            for x in 0..64 {
                let r = ((x * 2) % 255) as u8;
                let g = ((y * 2) % 255) as u8;
                rgba.extend_from_slice(&[r, g, 180, 255]);
            }
        }
        let png_b64 = encode_rgba_png_b64(64, 64, &rgba);
        let image_b64 = match options.format {
            ImageFormat::Png => png_b64.clone(),
            ImageFormat::Jpeg => {
                encode_rgba_image_b64(64, 64, &rgba, ImageFormat::Jpeg, options.quality)
            }
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
            image_format: options.format,
            image_base64: image_b64,
            png_base64: png_b64,
            capture_id: None,
            pixel_digest: None,
            ocr_boxes,
        })
    }

    fn render_som_image(
        &self,
        image_b64: &str,
        marks_json: &str,
        win_frame: &Rect,
        options: &ImageEncodeOptions,
    ) -> Result<String, crate::driver::DriverError> {
        render_som_image_to_b64(
            image_b64,
            marks_json,
            win_frame,
            options.format,
            options.quality,
            options.max_width,
        )
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
        let dir = std::env::temp_dir().join(format!("apexuse_gw_{}_{}", tag, std::process::id()));
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
        assert!(vis.image_b64.is_some());
        assert_eq!(vis.image_mime_type, "image/jpeg");
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
        let dir = std::env::temp_dir().join(format!("apexuse_gw_ro_{}", std::process::id()));
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
            encode_image: bool,
            options: &ImageEncodeOptions,
        ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
            MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
        }
        fn render_som_image(
            &self,
            b: &str,
            m: &str,
            wf: &Rect,
            options: &ImageEncodeOptions,
        ) -> Result<String, crate::driver::DriverError> {
            MockDriver::new().render_som_image(b, m, wf, options)
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
        let dir = std::env::temp_dir().join(format!("apexuse_gw_trunc_{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("apexuse_gw_direct_{}", std::process::id()));
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
            encode_image: bool,
            options: &ImageEncodeOptions,
        ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
            MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
        }
        fn render_som_image(
            &self,
            b: &str,
            m: &str,
            wf: &Rect,
            options: &ImageEncodeOptions,
        ) -> Result<String, crate::driver::DriverError> {
            MockDriver::new().render_som_image(b, m, wf, options)
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
        let dir = std::env::temp_dir().join(format!("apexuse_gw_tick_{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("apexuse_gw_unres_{}", std::process::id()));
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

        let dir = std::env::temp_dir().join(format!("apexuse_gw_batch_ok_{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("apexuse_gw_openurl_{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("apexuse_gw_replay_{}", std::process::id()));
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

    #[test]
    fn computer_click_accepts_coords_on_unfiltered_and_rejects_on_filtered_while_keeping_ref() {
        let dir = std::env::temp_dir().join(format!("apexuse_gw_clk_coord_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gw = Gateway::new(Box::new(MockDriver::new()), PolicySettings::default(), dir.clone());

        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.calculator".into()))]),
        );
        let ok_coord = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("x", JsonValue::Number(145.0)),
                        ("y", JsonValue::Number(240.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(ok_coord.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.calculator".into())),
                ("query", JsonValue::String("7".into())),
            ]),
        );
        let blocked_coord = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("x", JsonValue::Number(145.0)),
                        ("y", JsonValue::Number(240.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(blocked_coord.payload.get("code").and_then(|v| v.as_str()), Some("invalid_state"));

        let ok_ref = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(ok_ref.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn computer_wait_for_scopes_window_and_retries_transient_startup_errors() {
        struct TransientStartupDriver {
            calls: std::sync::atomic::AtomicUsize,
            seen_window: std::sync::Mutex<Option<i32>>,
        }
        impl OsDriver for TransientStartupDriver {
            fn doctor(&self) -> Result<DoctorStatus, crate::driver::DriverError> {
                MockDriver::new().doctor()
            }
            fn list_apps(&self) -> Result<Vec<crate::driver::RunningApp>, crate::driver::DriverError> {
                MockDriver::new().list_apps()
            }
            fn activate_app(&self, b: &str) -> Result<(), crate::driver::DriverError> {
                MockDriver::new().activate_app(b)
            }
            fn launch_app(&self, b: &str) -> Result<(), crate::driver::DriverError> {
                MockDriver::new().launch_app(b)
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                me: i32,
                md: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                *self.seen_window.lock().unwrap() = w;
                let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    return Err(crate::driver::DriverError::new("window_not_found", "Starting up"));
                }
                MockDriver::new().inspect_app(bundle_id, w, me, md)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn render_som_png(&self, b: &str, m: &str, wf: &Rect) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_png(b, m, wf)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                MockDriver::new().perform_action(req)
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_wait_win_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let drv = TransientStartupDriver {
            calls: std::sync::atomic::AtomicUsize::new(0),
            seen_window: std::sync::Mutex::new(None),
        };
        let mut gw = Gateway::new(Box::new(drv), PolicySettings::default(), dir.clone());
        let res = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("wait_for".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.apple.calculator".into())),
                        ("query", JsonValue::String("Notes".into())),
                        ("window", JsonValue::Number(0.0)),
                        ("timeout_ms", JsonValue::Number(800.0)),
                        ("poll_ms", JsonValue::Number(100.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(res.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.payload.get("source").and_then(|v| v.as_str()), Some("ax"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn computer_scroll_supports_ref_mark_and_coords_and_blocks_blind_filtered_scroll() {
        struct ScrollRecordingDriver {
            log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        }
        impl OsDriver for ScrollRecordingDriver {
            fn doctor(&self) -> Result<DoctorStatus, crate::driver::DriverError> {
                MockDriver::new().doctor()
            }
            fn list_apps(&self) -> Result<Vec<crate::driver::RunningApp>, crate::driver::DriverError> {
                MockDriver::new().list_apps()
            }
            fn activate_app(&self, b: &str) -> Result<(), crate::driver::DriverError> {
                MockDriver::new().activate_app(b)
            }
            fn launch_app(&self, b: &str) -> Result<(), crate::driver::DriverError> {
                MockDriver::new().launch_app(b)
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                me: i32,
                md: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                MockDriver::new().inspect_app(bundle_id, w, me, md)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn render_som_png(&self, b: &str, m: &str, wf: &Rect) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_png(b, m, wf)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.log.lock().unwrap().push(format!("{req:?}"));
                MockDriver::new().perform_action(req)
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_scroll_tgt_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut gw = Gateway::new(
            Box::new(ScrollRecordingDriver { log: log.clone() }),
            PolicySettings::default(),
            dir.clone(),
        );

        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.calculator".into()))]),
        );
        let sc_ref = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("scroll".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("direction", JsonValue::String("down".into())),
                        ("amount", JsonValue::Number(12.0)),
                        ("ref", JsonValue::String("e2".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(sc_ref.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let _ = gw.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![
                ("app", JsonValue::String("Calculator".into())),
                ("ocr", JsonValue::String("off".into())),
            ]),
        );
        let sc_coord = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("scroll".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("direction", JsonValue::String("up".into())),
                        ("amount", JsonValue::Number(5.0)),
                        ("x", JsonValue::Number(210.0)),
                        ("y", JsonValue::Number(240.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(sc_coord.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.calculator".into())),
                ("query", JsonValue::String("7".into())),
            ]),
        );
        let blind_filtered = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("scroll".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("direction", JsonValue::String("down".into())),
                        ("amount", JsonValue::Number(4.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(
            blind_filtered.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_state")
        );

        let ref_filtered = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("scroll".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("direction", JsonValue::String("down".into())),
                        ("amount", JsonValue::Number(4.0)),
                        ("ref", JsonValue::String("e1".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(ref_filtered.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let entries = log.lock().unwrap();
        assert!(entries[0].contains("x: Some(230.0), y: Some(340.0)"));
        assert!(entries[1].contains("x: Some(210.0), y: Some(240.0)"));
        assert!(entries[2].contains("x: Some(145.0), y: Some(240.0)"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn action_verify_change_and_auto_retry_handles_pixel_and_strict_failure() {
        struct PixelFlakyDriver {
            actions: std::sync::atomic::AtomicUsize,
            succeed_on_attempt: usize,
        }
        impl OsDriver for PixelFlakyDriver {
            fn doctor(&self) -> Result<DoctorStatus, crate::driver::DriverError> {
                MockDriver::new().doctor()
            }
            fn list_apps(&self) -> Result<Vec<crate::driver::RunningApp>, crate::driver::DriverError> {
                MockDriver::new().list_apps()
            }
            fn activate_app(&self, b: &str) -> Result<(), crate::driver::DriverError> {
                MockDriver::new().activate_app(b)
            }
            fn launch_app(&self, b: &str) -> Result<(), crate::driver::DriverError> {
                MockDriver::new().launch_app(b)
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                me: i32,
                md: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                MockDriver::new().inspect_app(bundle_id, w, me, md)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                let mut cap = MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)?;
                let count = self.actions.load(std::sync::atomic::Ordering::SeqCst);
                let byte = if count >= self.succeed_on_attempt { "ff" } else { "10" };
                cap.pixel_digest = Some(byte.repeat(256));
                Ok(cap)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn render_som_png(&self, b: &str, m: &str, wf: &Rect) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_png(b, m, wf)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.actions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                MockDriver::new().perform_action(req)
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_verify_retry_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut gw_retry = Gateway::new(
            Box::new(PixelFlakyDriver {
                actions: std::sync::atomic::AtomicUsize::new(0),
                succeed_on_attempt: 2,
            }),
            PolicySettings::default(),
            dir.clone(),
        );
        let _ = gw_retry.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.calculator".into()))]),
        );
        let res = gw_retry.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("verify_change", JsonValue::Bool(true)),
                        ("max_retries", JsonValue::Number(2.0)),
                        ("settle_ms", JsonValue::Number(10.0)),
                        ("retry_delay_ms", JsonValue::Number(10.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(res.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.payload.get("verified_change").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.payload.get("visual_changed").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.payload.get("ax_changed").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(res.payload.get("attempts").and_then(|v| v.as_i64()), Some(2));
        assert_eq!(res.payload.get("retried").and_then(|v| v.as_i64()), Some(1));

        // Strict require_change fails cleanly when neither AX nor pixels ever change
        let mut gw_strict = Gateway::new(
            Box::new(PixelFlakyDriver {
                actions: std::sync::atomic::AtomicUsize::new(0),
                succeed_on_attempt: 99,
            }),
            PolicySettings::default(),
            dir.clone(),
        );
        let _ = gw_strict.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.calculator".into()))]),
        );
        let batch_fail = gw_strict.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "steps",
                        JsonValue::Array(vec![JsonValue::object(vec![
                            ("action", JsonValue::String("scroll".into())),
                            ("direction", JsonValue::String("down".into())),
                            ("amount", JsonValue::Number(5.0)),
                            ("ref", JsonValue::String("e2".into())),
                            ("max_retries", JsonValue::Number(1.0)),
                            ("settle_ms", JsonValue::Number(10.0)),
                            ("retry_delay_ms", JsonValue::Number(10.0)),
                            ("require_change", JsonValue::Bool(true)),
                        ])]),
                    )]),
                ),
            ]),
        );
        assert_eq!(batch_fail.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            batch_fail.payload.get("code").and_then(|v| v.as_str()),
            Some("no_change_detected")
        );
        assert_eq!(batch_fail.payload.get("failed_step").and_then(|v| v.as_i64()), Some(0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_value_and_select_text_safety_clearing_and_memory_replay() {
        use std::sync::{Arc, Mutex};
        struct LogDriver(Arc<Mutex<Vec<String>>>);
        impl OsDriver for LogDriver {
            fn doctor(&self) -> Result<DoctorStatus, crate::driver::DriverError> {
                MockDriver::new().doctor()
            }
            fn list_apps(&self) -> Result<Vec<crate::driver::RunningApp>, crate::driver::DriverError> {
                MockDriver::new().list_apps()
            }
            fn activate_app(&self, b: &str) -> Result<(), crate::driver::DriverError> {
                MockDriver::new().activate_app(b)
            }
            fn launch_app(&self, b: &str) -> Result<(), crate::driver::DriverError> {
                MockDriver::new().launch_app(b)
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                me: i32,
                md: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                MockDriver::new().inspect_app(bundle_id, w, me, md)
            }
            fn window_state(&self, bundle_id: &str) -> Result<WindowState, crate::driver::DriverError> {
                MockDriver::new().window_state(bundle_id)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.0.lock().unwrap().push(format!("{req:?}"));
                MockDriver::new().perform_action(req)
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_setval_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut cfg = crate::config::AppConfig::default();
        cfg.timing.key_delay_us = 250;
        let mut gw = Gateway::with_config(
            Box::new(LogDriver(log.clone())),
            PolicySettings::default(),
            dir.clone(),
            cfg,
        );

        let vis = gw.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        assert_eq!(vis.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // 1. set_value succeeds on TextArea ref (including "" clearing) and editable AX mark m3
        let sv_set = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("native text".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(sv_set.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let sv_clear = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("mark", JsonValue::String("m3".into())),
                        ("text", JsonValue::String("".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(sv_clear.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // 2. type still rejects empty string ""
        let type_empty = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("type".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(
            type_empty.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_argument")
        );

        // 3. Fail-closed target_identity_unverified on raw (x, y), Button ref e1, non-editable mark m1, and OCR-only mark m2
        for (act, bad_args) in [
            (
                "set_value",
                JsonValue::object(vec![
                    ("x", JsonValue::Number(145.0)),
                    ("y", JsonValue::Number(240.0)),
                    ("text", JsonValue::String("x".into())),
                ]),
            ),
            (
                "set_value",
                JsonValue::object(vec![
                    ("ref", JsonValue::String("e1".into())),
                    ("text", JsonValue::String("x".into())),
                ]),
            ),
            (
                "set_value",
                JsonValue::object(vec![
                    ("mark", JsonValue::String("m2".into())),
                    ("text", JsonValue::String("x".into())),
                ]),
            ),
            (
                "select_text",
                JsonValue::object(vec![
                    ("x", JsonValue::Number(145.0)),
                    ("y", JsonValue::Number(240.0)),
                ]),
            ),
            (
                "select_text",
                JsonValue::object(vec![("mark", JsonValue::String("m2".into()))]),
            ),
            (
                "select_text",
                JsonValue::object(vec![("ref", JsonValue::String("e1".into()))]),
            ),
        ] {
            let res = gw.dispatch(
                "computer",
                &JsonValue::object(vec![
                    ("action", JsonValue::String(act.into())),
                    ("arguments", bad_args),
                ]),
            );
            assert_eq!(
                res.payload.get("code").and_then(|v| v.as_str()),
                Some("target_identity_unverified"),
                "expected fail-closed for {act}"
            );
        }

        // 4. select_text succeeds on e2 with start <= end, rejects start > end
        let sel_bad = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("select_text".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("start", JsonValue::Number(5.0)),
                        ("end", JsonValue::Number(2.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(sel_bad.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        let sel_ok = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("select_text".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("start", JsonValue::Number(1.0)),
                        ("end", JsonValue::Number(4.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(sel_ok.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // 5. workflow_memory parameterizes set_value without storing literal secret text and replays SetValue
        let rec = gw.dispatch(
            "workflow_memory",
            &JsonValue::object(vec![
                ("action", JsonValue::String("record".into())),
                ("app", JsonValue::String("com.apple.calculator".into())),
                ("goal", JsonValue::String("fill notes natively".into())),
                (
                    "steps",
                    JsonValue::Array(vec![JsonValue::object(vec![
                        ("action", JsonValue::String("set_value".into())),
                        ("target_role", JsonValue::String("TextArea".into())),
                        ("target_label", JsonValue::String("Notes".into())),
                        ("text", JsonValue::String("should_never_persist".into())),
                    ])]),
                ),
            ]),
        );
        assert_eq!(rec.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        let rec_str = rec.payload.to_json_string();
        assert!(!rec_str.contains("should_never_persist"));

        let rep = gw.dispatch(
            "workflow_memory",
            &JsonValue::object(vec![
                ("action", JsonValue::String("replay".into())),
                ("app", JsonValue::String("com.apple.calculator".into())),
                ("goal", JsonValue::String("fill notes".into())),
                (
                    "step_args",
                    JsonValue::Array(vec![JsonValue::object(vec![(
                        "text",
                        JsonValue::String("replayed native".into()),
                    )])]),
                ),
            ]),
        );
        assert_eq!(rep.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let entries = log.lock().unwrap();
        assert!(entries.iter().any(|e| e.contains("SetValue") && e.contains("native text") && e.contains("key_delay_us: 250")));
        assert!(entries.iter().any(|e| e.contains("SetValue") && e.contains("text: \"\"")));
        assert!(entries.iter().any(|e| e.contains("SelectText") && e.contains("start: Some(1)") && e.contains("end: Some(4)")));
        assert!(entries.iter().any(|e| e.contains("SetValue") && e.contains("replayed native")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn semantic_target_reuses_snapshot_disambiguates_and_fails_closed() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        };
        struct SemanticDriver {
            inspect_calls: Arc<AtomicUsize>,
            actions: Arc<Mutex<Vec<String>>>,
        }
        impl OsDriver for SemanticDriver {
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
            fn open_url(&self, _: &str, _: &str) -> Result<(), crate::driver::DriverError> {
                Ok(())
            }
            fn supports_fast_point_verify(&self) -> bool {
                true
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                _: Option<i32>,
                _: i32,
                _: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                let n = self.inspect_calls.fetch_add(1, Ordering::SeqCst) + 1;
                let elem = |ref_id: &str, role: &str, label: &str, x: f64, y: f64| UiElement {
                    ref_id: ref_id.into(),
                    role: role.into(),
                    label: label.into(),
                    value: None,
                    url: None,
                    window: 0,
                    depth: 1,
                    interactive: true,
                    enabled: Some(true),
                    focused: false,
                    actions: vec!["AXPress".into()],
                    frame: Some(Rect { x, y, w: 80.0, h: 30.0 }),
                };
                let mut elements = vec![
                    elem("e1", "Button", "Save", 100.0, 100.0),
                    elem("e2", "Button", "Save As...", 200.0, 100.0),
                    elem("e3", "Button", "Delete", 100.0, 160.0),
                    elem("e4", "Button", "Delete", 200.0, 160.0),
                    elem("e5", "Button", "Export PDF", 100.0, 220.0),
                    elem("e6", "Button", "Export PNG", 200.0, 220.0),
                    elem("e7", "TextField", "Title", 100.0, 280.0),
                ];
                if n >= 2 {
                    elements.push(elem("e8", "Button", "LateItem", 300.0, 100.0));
                }
                Ok(AppSnapshot {
                    bundle_id: bundle_id.into(),
                    frontmost_bundle_id: bundle_id.into(),
                    pid: 777,
                    frontmost: true,
                    windows_total: 1,
                    windows: vec![crate::driver::WindowMeta {
                        index: 0,
                        title: "Doc".into(),
                        frame: Some(Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }),
                    }],
                    elements,
                    has_secure_field: false,
                    has_unverified_field: false,
                    truncated: false,
                    depth_truncated: false,
                    safety_scan_complete: true,
                })
            }
            fn window_state(&self, bundle_id: &str) -> Result<WindowState, crate::driver::DriverError> {
                Ok(WindowState {
                    frontmost_bundle_id: bundle_id.into(),
                    pid: 777,
                    windows_total: 1,
                    active_window_frame: Some(Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }),
                })
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.actions.lock().unwrap().push(format!("{req:?}"));
                Ok(crate::driver::ActionResult { method: "ax".into() })
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_sem_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let actions = Arc::new(Mutex::new(Vec::new()));
        let mut gw = Gateway::new(
            Box::new(SemanticDriver {
                inspect_calls: calls.clone(),
                actions: actions.clone(),
            }),
            PolicySettings::default(),
            dir.clone(),
        );

        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 1. set_value with semantic target reuses cached unfiltered snapshot (inspect_calls stays 1) and preserves snapshot
        let title_set = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("target", JsonValue::object(vec![("query", JsonValue::String("Title".into()))])),
                        ("text", JsonValue::String("Draft".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(title_set.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 2. Active cached lookup misses "LateItem" -> performs 1 automatic re-inspect (inspect_calls == 2) -> succeeds
        let late_clk = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "target",
                        JsonValue::object(vec![("query", JsonValue::String("LateItem".into()))]),
                    )]),
                ),
            ]),
        );
        assert_eq!(late_clk.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // 3. Re-inspect and verify exact case-insensitive match "save" beats "Save As..." with 0 extra inspect calls
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let save_clk = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "target",
                        JsonValue::object(vec![
                            ("query", JsonValue::String("save".into())),
                            ("role", JsonValue::String("button".into())),
                        ]),
                    )]),
                ),
            ]),
        );
        assert_eq!(save_clk.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        // 4. fresh=true forces 1 fresh inspect even when snapshot is active
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        let before_fresh = calls.load(Ordering::SeqCst);
        let fresh_set = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        (
                            "target",
                            JsonValue::object(vec![
                                ("query", JsonValue::String("Title".into())),
                                ("fresh", JsonValue::Bool(true)),
                            ]),
                        ),
                        ("text", JsonValue::String("Fresh".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(fresh_set.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(calls.load(Ordering::SeqCst), before_fresh + 1);

        // Rejects empty target.query and ambiguous combining target+ref
        let empty_q = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![("target", JsonValue::object(vec![("query", JsonValue::String("   ".into()))]))]),
                ),
            ]),
        );
        assert_eq!(empty_q.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        let combo = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("target", JsonValue::object(vec![("query", JsonValue::String("Save".into()))])),
                    ]),
                ),
            ]),
        );
        assert_eq!(combo.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        // Non-editable target on set_value fails closed with target_identity_unverified
        let non_edit = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("target", JsonValue::object(vec![("query", JsonValue::String("Save".into()))])),
                        ("text", JsonValue::String("bad".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(non_edit.payload.get("code").and_then(|v| v.as_str()), Some("target_identity_unverified"));

        // 3. Multiple exact matches ("Delete") and multiple partial matches ("Export") fail closed with ambiguous_target
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        let amb_exact = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "target",
                        JsonValue::object(vec![("query", JsonValue::String("Delete".into()))]),
                    )]),
                ),
            ]),
        );
        assert_eq!(amb_exact.payload.get("code").and_then(|v| v.as_str()), Some("ambiguous_target"));

        let amb_partial = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "target",
                        JsonValue::object(vec![("query", JsonValue::String("Export".into()))]),
                    )]),
                ),
            ]),
        );
        assert_eq!(amb_partial.payload.get("code").and_then(|v| v.as_str()), Some("ambiguous_target"));

        // 4. Non-existent target refreshes once and returns target_not_found
        let before_missing = calls.load(Ordering::SeqCst);
        let not_found = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "target",
                        JsonValue::object(vec![("query", JsonValue::String("DoesNotExist".into()))]),
                    )]),
                ),
            ]),
        );
        assert_eq!(not_found.payload.get("code").and_then(|v| v.as_str()), Some("target_not_found"));
        assert_eq!(calls.load(Ordering::SeqCst), before_missing + 1);

        let log = actions.lock().unwrap();
        assert!(log.iter().any(|e| e.contains("x: 140.0, y: 130.0") || e.contains("x: 140.0, y: 115.0")));
        assert!(log.iter().any(|e| e.contains("x: 340.0, y: 115.0")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn expanded_run_actions_supports_24_steps_with_inline_wait_set_value_select_and_target() {
        let dir = std::env::temp_dir().join(format!("apexuse_gw_batch24_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gw = Gateway::new(Box::new(MockDriver::new()), PolicySettings::default(), dir.clone());

        let mut steps = Vec::new();
        steps.push(JsonValue::object(vec![
            ("action", JsonValue::String("wait_for".into())),
            ("text", JsonValue::String("Notes".into())),
            ("role", JsonValue::String("TextArea".into())),
            ("timeout_ms", JsonValue::Number(500.0)),
        ]));
        steps.push(JsonValue::object(vec![
            ("action", JsonValue::String("set_value".into())),
            ("target", JsonValue::object(vec![("query", JsonValue::String("Notes".into()))])),
            ("text", JsonValue::String("seed".into())),
        ]));
        steps.push(JsonValue::object(vec![
            ("action", JsonValue::String("select_text".into())),
            ("ref", JsonValue::String("e2".into())),
            ("start", JsonValue::Number(0.0)),
            ("end", JsonValue::Number(4.0)),
        ]));
        for i in 0..10 {
            steps.push(JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                ("ref", JsonValue::String("e2".into())),
                ("text", JsonValue::String(format!("step_{i}"))),
            ]));
        }
        steps.push(JsonValue::object(vec![
            ("action", JsonValue::String("click".into())),
            (
                "target",
                JsonValue::object(vec![
                    ("query", JsonValue::String("7".into())),
                    ("role", JsonValue::String("Button".into())),
                ]),
            ),
        ]));
        assert_eq!(steps.len(), 14);

        let ok = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("Calculator".into())),
                        ("steps", JsonValue::Array(steps)),
                    ]),
                ),
            ]),
        );
        assert_eq!(ok.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ok.payload.get("executed").and_then(|v| v.as_i64()), Some(14));

        // 25 steps must still be rejected
        let too_many: Vec<JsonValue> = (0..25)
            .map(|_| {
                JsonValue::object(vec![
                    ("action", JsonValue::String("press_key".into())),
                    ("key", JsonValue::String("tab".into())),
                ])
            })
            .collect();
        let err = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("Calculator".into())),
                        ("steps", JsonValue::Array(too_many)),
                    ]),
                ),
            ]),
        );
        assert_eq!(err.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn adaptive_verify_change_exits_early_on_first_poll_tick() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Tick1Driver {
            ticks: Arc<AtomicUsize>,
            acted: Arc<AtomicUsize>,
        }
        impl OsDriver for Tick1Driver {
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
                if self.acted.load(Ordering::SeqCst) > 0 && self.ticks.load(Ordering::SeqCst) >= 1 {
                    if let Some(el) = snap.elements.iter_mut().find(|e| e.ref_id == "e2") {
                        el.value = Some("updated".into());
                    }
                }
                Ok(snap)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.acted.fetch_add(1, Ordering::SeqCst);
                MockDriver::new().perform_action(req)
            }
            fn tick(&self) {
                if self.acted.load(Ordering::SeqCst) > 0 {
                    self.ticks.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_adaptive_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gw = Gateway::new(
            Box::new(Tick1Driver {
                ticks: Arc::new(AtomicUsize::new(0)),
                acted: Arc::new(AtomicUsize::new(0)),
            }),
            PolicySettings::default(),
            dir.clone(),
        );
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );

        let t0 = Instant::now();
        let res = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("verify_change", JsonValue::Bool(true)),
                        ("settle_ms", JsonValue::Number(800.0)),
                        ("poll_interval_ms", JsonValue::Number(30.0)),
                    ]),
                ),
            ]),
        );
        let elapsed = t0.elapsed();
        assert_eq!(res.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.payload.get("verified_change").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.payload.get("ax_changed").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.payload.get("attempts").and_then(|v| v.as_i64()), Some(1));
        assert!(
            elapsed < Duration::from_millis(350),
            "expected early exit well under settle_ms=800ms, got {:?}",
            elapsed
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn window_state_preserves_form_refs_and_marks_across_edits_and_invalidates_on_focus_change() {
        use std::sync::{Arc, Mutex};
        struct FocusSwitchDriver {
            frontmost: Arc<Mutex<String>>,
        }
        impl OsDriver for FocusSwitchDriver {
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
                snap.frontmost_bundle_id = self.frontmost.lock().unwrap().clone();
                Ok(snap)
            }
            fn window_state(&self, _: &str) -> Result<WindowState, crate::driver::DriverError> {
                Ok(WindowState {
                    frontmost_bundle_id: self.frontmost.lock().unwrap().clone(),
                    pid: 4242,
                    windows_total: 1,
                    active_window_frame: Some(Rect {
                        x: 100.0,
                        y: 100.0,
                        w: 320.0,
                        h: 440.0,
                    }),
                })
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                let mut cap = MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)?;
                cap.frontmost_bundle_id = self.frontmost.lock().unwrap().clone();
                Ok(cap)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                MockDriver::new().perform_action(req)
            }
        }

        // Case 1: Background app + confirmation: required -> preserves refs & marks while consuming single-use tokens
        let dir = std::env::temp_dir().join(format!("apexuse_gw_form_reuse_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front = Arc::new(Mutex::new("com.apple.finder".to_string()));
        let settings = PolicySettings {
            require_confirmation: true,
            allowed_apps: vec!["*".into()],
            ..PolicySettings::default()
        };
        let mut gw = Gateway::new(
            Box::new(FocusSwitchDriver { frontmost: front.clone() }),
            settings,
            dir.clone(),
        );

        let vis = gw.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(vis.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let req1 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("first".into())),
                    ]),
                ),
            ]),
        );
        let tok1 = req1.payload.get("token").and_then(|v| v.as_str()).unwrap().to_string();
        let conf1 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                ("arguments", JsonValue::object(vec![("token", JsonValue::String(tok1.clone()))])),
            ]),
        );
        assert_eq!(conf1.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // Consumed token cannot be replayed even though snapshot was preserved
        let replay1 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                ("arguments", JsonValue::object(vec![("token", JsonValue::String(tok1))])),
            ]),
        );
        assert_eq!(
            replay1.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_confirmation")
        );

        // Mark m3 and ref e2 remain valid without re-inspecting!
        let req2 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("select_text".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("mark", JsonValue::String("m3".into())),
                        ("start", JsonValue::Number(0.0)),
                        ("end", JsonValue::Number(5.0)),
                    ]),
                ),
            ]),
        );
        let tok2 = req2.payload.get("token").and_then(|v| v.as_str()).unwrap().to_string();
        let conf2 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                ("arguments", JsonValue::object(vec![("token", JsonValue::String(tok2))])),
            ]),
        );
        assert_eq!(conf2.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // Now simulate frontmost_bundle_id changing mid-form before next edit executes
        *front.lock().unwrap() = "com.apple.Safari".into();
        let req3 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("type".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("after switch".into())),
                    ]),
                ),
            ]),
        );
        let tok3 = req3.payload.get("token").and_then(|v| v.as_str()).unwrap().to_string();
        let conf3 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                ("arguments", JsonValue::object(vec![("token", JsonValue::String(tok3))])),
            ]),
        );
        assert_eq!(conf3.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // Because frontmost_bundle_id changed, both elements_by_ref and marks_by_id must now be invalidated!
        let stale_ref = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("x".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(stale_ref.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));

        let stale_mark = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("type".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("mark", JsonValue::String("m3".into())),
                        ("text", JsonValue::String("x".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(stale_mark.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_options_validation_empty_marks_transcode_and_config_keep_snapshot() {
        struct LegacyPngEmptyDriver;
        impl OsDriver for LegacyPngEmptyDriver {
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
                snap.elements.clear();
                Ok(snap)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                _: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                let mut cap = MockDriver::new().capture_and_ocr(bundle_id, w, OcrMode::Off, encode_image, options)?;
                // Simulate a legacy driver returning PNG regardless of options.format
                cap.image_format = ImageFormat::Png;
                cap.image_base64 = cap.png_base64.clone();
                Ok(cap)
            }
            fn render_som_image(
                &self,
                image_b64: &str,
                _marks_json: &str,
                _win_frame: &Rect,
                _options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                Ok(image_b64.to_string())
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                MockDriver::new().perform_action(req)
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_img_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut gw_empty = Gateway::new(Box::new(LegacyPngEmptyDriver), PolicySettings::default(), dir.clone());
        let vis = gw_empty.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![
                ("app", JsonValue::String("Calculator".into())),
                ("ocr", JsonValue::String("off".into())),
            ]),
        );
        assert_eq!(vis.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(vis.payload.get("marks_count").and_then(|v| v.as_i64()), Some(0));
        assert_eq!(vis.image_mime_type, "image/jpeg");
        assert_eq!(vis.payload.get("image_format").and_then(|v| v.as_str()), Some("jpeg"));
        let decoded = base64_decode(vis.image_b64.as_deref().unwrap()).unwrap();
        assert!(decoded.starts_with(&[0xFF, 0xD8, 0xFF]));

        // Validation errors on bad format / quality / max_width
        for bad in [
            JsonValue::object(vec![("app", JsonValue::String("Calculator".into())), ("format", JsonValue::String("webp".into()))]),
            JsonValue::object(vec![("app", JsonValue::String("Calculator".into())), ("quality", JsonValue::Number(0.0))]),
            JsonValue::object(vec![("app", JsonValue::String("Calculator".into())), ("quality", JsonValue::Number(101.0))]),
            JsonValue::object(vec![("app", JsonValue::String("Calculator".into())), ("max_width", JsonValue::Number(639.0))]),
            JsonValue::object(vec![("app", JsonValue::String("Calculator".into())), ("max_width", JsonValue::Number(2561.0))]),
        ] {
            let err_v = gw_empty.dispatch("visual_inspect", &bad);
            assert_eq!(err_v.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));
            let err_s = gw_empty.dispatch("screenshot", &bad);
            assert_eq!(err_s.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));
        }

        // Gateway::with_config reuse_form_refs = false vs explicit keep_snapshot = true
        let mut cfg = AppConfig::default();
        cfg.snapshot.reuse_form_refs = false;
        let mut gw_no_reuse = Gateway::with_config(
            Box::new(MockDriver::new()),
            PolicySettings::default(),
            dir.clone(),
            cfg,
        );
        let _ = gw_no_reuse.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        // Explicit keep_snapshot: true keeps refs even when config.reuse_form_refs == false
        let keep_res = gw_no_reuse.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("a".into())),
                        ("keep_snapshot", JsonValue::Bool(true)),
                    ]),
                ),
            ]),
        );
        assert_eq!(keep_res.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(keep_res.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(true));

        // Now without keep_snapshot -> reuse_form_refs == false invalidates
        let drop_res = gw_no_reuse.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("b".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(drop_res.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(drop_res.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(false));

        let stale = gw_no_reuse.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("c".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(stale.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn semantic_target_refresh_failure_invalidates_prior_refs_and_marks() {
        use std::sync::{
            atomic::{AtomicU8, Ordering},
            Arc,
        };
        struct EscalatingDriver {
            // 0 = normal, 1 = secure field appears, 2 = protected frontmost app
            mode: Arc<AtomicU8>,
        }
        impl OsDriver for EscalatingDriver {
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
            fn open_url(&self, _: &str, _: &str) -> Result<(), crate::driver::DriverError> {
                Ok(())
            }
            fn supports_fast_point_verify(&self) -> bool {
                true
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                me: i32,
                md: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                let mut snap = MockDriver::new().inspect_app(bundle_id, w, me, md)?;
                match self.mode.load(Ordering::SeqCst) {
                    1 => snap.has_secure_field = true,
                    2 => snap.frontmost_bundle_id = "com.1password.app".into(),
                    _ => {}
                }
                Ok(snap)
            }
            fn window_state(&self, bundle_id: &str) -> Result<WindowState, crate::driver::DriverError> {
                MockDriver::new().window_state(bundle_id)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                MockDriver::new().perform_action(req)
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_sem_fail_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Case A: refresh encounters secure field -> unsafe_or_incomplete_tree invalidates prior ref & mark
        let mode_a = Arc::new(AtomicU8::new(0));
        let mut gw_sec = Gateway::new(
            Box::new(EscalatingDriver { mode: mode_a.clone() }),
            PolicySettings::default(),
            dir.clone(),
        );
        let vis_a = gw_sec.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        assert_eq!(vis_a.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        mode_a.store(1, Ordering::SeqCst);
        let err_sec = gw_sec.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "target",
                        JsonValue::object(vec![
                            ("query", JsonValue::String("7".into())),
                            ("fresh", JsonValue::Bool(true)),
                        ]),
                    )]),
                ),
            ]),
        );
        assert_eq!(
            err_sec.payload.get("code").and_then(|v| v.as_str()),
            Some("unsafe_or_incomplete_tree")
        );

        let stale_ref_a = gw_sec.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(stale_ref_a.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));

        let stale_mark_a = gw_sec.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("mark", JsonValue::String("m3".into()))])),
            ]),
        );
        assert_eq!(stale_mark_a.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));

        // Case B: refresh encounters protected frontmost app -> invalid_app invalidates prior ref & mark
        let mode_b = Arc::new(AtomicU8::new(0));
        let mut gw_prot = Gateway::new(
            Box::new(EscalatingDriver { mode: mode_b.clone() }),
            PolicySettings::default(),
            dir.clone(),
        );
        let vis_b = gw_prot.dispatch(
            "visual_inspect",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        assert_eq!(vis_b.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        mode_b.store(2, Ordering::SeqCst);
        let err_prot = gw_prot.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "target",
                        JsonValue::object(vec![("query", JsonValue::String("MissingInCache".into()))]),
                    )]),
                ),
            ]),
        );
        assert_eq!(err_prot.payload.get("code").and_then(|v| v.as_str()), Some("invalid_app"));

        let stale_ref_b = gw_prot.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(stale_ref_b.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));

        let stale_mark_b = gw_prot.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("mark", JsonValue::String("m3".into()))])),
            ]),
        );
        assert_eq!(stale_mark_b.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_app_ax_diff_bounds_churn_and_security_redaction() {
        use std::collections::VecDeque;
        use std::sync::{Arc, Mutex};

        struct SeqSnapshotDriver {
            snaps: Arc<Mutex<VecDeque<AppSnapshot>>>,
        }
        impl OsDriver for SeqSnapshotDriver {
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
                _bundle_id: &str,
                _window: Option<i32>,
                _me: i32,
                _md: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                let mut guard = self.snaps.lock().unwrap();
                if guard.len() > 1 {
                    Ok(guard.pop_front().unwrap())
                } else {
                    Ok(guard.front().unwrap().clone())
                }
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                MockDriver::new().perform_action(req)
            }
        }

        let mk_el = |ref_id: &str, role: &str, label: &str, val: Option<&str>, x: f64| UiElement {
            ref_id: ref_id.into(),
            role: role.into(),
            label: label.into(),
            value: val.map(str::to_string),
            url: None,
            window: 0,
            depth: 1,
            interactive: !ref_id.is_empty(),
            enabled: Some(true),
            focused: false,
            actions: vec!["AXPress".into()],
            frame: Some(Rect { x, y: 100.0, w: 80.0, h: 30.0 }),
        };
        let mk_snap = |els: Vec<UiElement>, secure: bool| AppSnapshot {
            bundle_id: "com.apple.TextEdit".into(),
            frontmost_bundle_id: "com.apple.TextEdit".into(),
            pid: 5150,
            frontmost: true,
            windows_total: 1,
            windows: vec![crate::driver::WindowMeta {
                index: 0,
                title: "Doc".into(),
                frame: Some(Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }),
            }],
            elements: els,
            has_secure_field: secure,
            has_unverified_field: false,
            truncated: false,
            depth_truncated: false,
            safety_scan_complete: true,
        };

        let s1 = mk_snap(
            vec![
                mk_el("e1", "Button", "7", None, 20.0),
                mk_el("e2", "TextArea", "Notes", Some(""), 120.0),
                mk_el("e3", "Button", "Reset", None, 220.0),
                mk_el("", "StaticText", "Ready", None, 320.0),
            ],
            false,
        );
        let s2_unchanged = s1.clone();
        let s3_changed = mk_snap(
            vec![
                mk_el("e1", "Button", "7", None, 20.0),
                mk_el("e2", "TextArea", "Notes", Some("hello"), 120.0),
                mk_el("e3", "Button", "Share", None, 320.0),
                mk_el("", "StaticText", "Ready", None, 420.0),
            ],
            false,
        );
        let s4_filtered_check = s3_changed.clone();
        let s5_unchanged_after_filter = s3_changed.clone();
        let s6_explicit_full_bounds = s3_changed.clone();
        let s7_huge_churn = mk_snap(
            (1..=6)
                .map(|i| mk_el(&format!("e{i}"), "Button", &format!("Fresh{i}"), None, i as f64 * 40.0))
                .collect(),
            false,
        );
        let s8_secure_hidden_by_filter = mk_snap(
            vec![
                mk_el("e1", "Button", "Other", None, 20.0),
                mk_el("e2", "TextField", "Password Field", Some("••••"), 120.0),
            ],
            false,
        );
        let s9_clean_after_redaction = mk_snap(
            vec![mk_el("e1", "Button", "CleanAgain", None, 20.0)],
            false,
        );

        let snaps = Arc::new(Mutex::new(VecDeque::from(vec![
            s1,
            s2_unchanged,
            s3_changed,
            s4_filtered_check,
            s5_unchanged_after_filter,
            s6_explicit_full_bounds,
            s7_huge_churn,
            s8_secure_hidden_by_filter,
            s9_clean_after_redaction,
        ])));

        let dir = std::env::temp_dir().join(format!("apexuse_gw_axdiff_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gw = Gateway::new(
            Box::new(SeqSnapshotDriver { snaps }),
            PolicySettings::default(),
            dir.clone(),
        );

        // 1. First inspect -> full, compact (no bounds by default)
        let r1 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(r1.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(r1.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));
        let c1 = r1.payload.get("content").and_then(|v| v.as_str()).unwrap();
        assert!(!c1.contains("@("));
        assert!(c1.contains("[e1] Button \"7\""));

        // 2. Unchanged -> unchanged summary
        let r2 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(r2.payload.get("state_mode").and_then(|v| v.as_str()), Some("unchanged"));
        let c2 = r2.payload.get("content").and_then(|v| v.as_str()).unwrap();
        assert!(c2.contains("unchanged"));
        assert!(!c2.contains("[e1] Button \"7\""));

        // 3. Modified Notes value, removed e3 (Reset), added Share -> diff with monotonic e4
        let r3 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(r3.payload.get("state_mode").and_then(|v| v.as_str()), Some("diff"));
        let c3 = r3.payload.get("content").and_then(|v| v.as_str()).unwrap();
        assert!(c3.contains("Removed refs: e3"), "got: {c3}");
        assert!(c3.contains("~ [e2] TextArea \"Notes\" value=\"hello\""), "got: {c3}");
        assert!(c3.contains("+ [e4] Button \"Share\""), "got: {c3}");
        assert!(!c3.contains("@("));
        assert!(!c3.contains("[e1] Button \"7\""));

        // 4. Filtered inspect returns state_mode "filtered" without clobbering diff baseline
        let r4 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.TextEdit".into())),
                ("query", JsonValue::String("Notes".into())),
            ]),
        );
        assert_eq!(r4.payload.get("state_mode").and_then(|v| v.as_str()), Some("filtered"));

        // 5. Repeat unfiltered inspect after filtered inspect still reports unchanged
        let r5 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(r5.payload.get("state_mode").and_then(|v| v.as_str()), Some("unchanged"));

        // 6. Explicit diff: false + include_bounds: true
        let r6 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.TextEdit".into())),
                ("diff", JsonValue::Bool(false)),
                ("include_bounds", JsonValue::Bool(true)),
            ]),
        );
        assert_eq!(r6.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));
        let c6 = r6.payload.get("content").and_then(|v| v.as_str()).unwrap();
        assert!(c6.contains("@(20,100,80x30)"), "got: {c6}");

        // 7. >80% churn resets cleanly to full
        let r7 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(r7.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));

        // 8. Security redaction checks FULL unfiltered outline even when query filters out the secret field!
        let r8 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.TextEdit".into())),
                ("query", JsonValue::String("Other".into())),
            ]),
        );
        assert_eq!(
            r8.payload.get("content").and_then(|v| v.as_str()),
            Some("[redacted: secure field detected]")
        );
        assert!(r8.payload.get("redaction_reason").is_some());

        // 9. After redaction cleared prev_ax_snapshot, next clean inspect starts with full
        let r9 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(r9.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));

        // Invalid types for diff / include_bounds fail with invalid_argument
        let bad_diff = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.TextEdit".into())),
                ("diff", JsonValue::String("yes".into())),
            ]),
        );
        assert_eq!(bad_diff.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        let bad_bounds = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("com.apple.TextEdit".into())),
                ("include_bounds", JsonValue::Number(1.0)),
            ]),
        );
        assert_eq!(bad_bounds.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn inline_return_state_keeps_refs_live_and_redacts_on_unsafe_tree() {
        use std::sync::{
            atomic::{AtomicU8, AtomicUsize, Ordering},
            Arc, Mutex,
        };

        struct PostActionDriver {
            inspect_calls: Arc<AtomicUsize>,
            actions: Arc<Mutex<Vec<String>>>,
            // 0 = initial, 1 = post-click new button, 2 = secure field, 3 = protected frontmost, 4 = inspect err
            phase: Arc<AtomicU8>,
        }
        impl OsDriver for PostActionDriver {
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
            fn open_url(&self, _: &str, _: &str) -> Result<(), crate::driver::DriverError> {
                Ok(())
            }
            fn supports_fast_point_verify(&self) -> bool {
                true
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                _: Option<i32>,
                _: i32,
                _: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                self.inspect_calls.fetch_add(1, Ordering::SeqCst);
                let p = self.phase.load(Ordering::SeqCst);
                if p == 4 {
                    return Err(crate::driver::DriverError::new("window_not_found", "Window closed"));
                }
                let mk = |ref_id: &str, role: &str, label: &str, val: Option<&str>, x: f64| UiElement {
                    ref_id: ref_id.into(),
                    role: role.into(),
                    label: label.into(),
                    value: val.map(str::to_string),
                    url: None,
                    window: 0,
                    depth: 1,
                    interactive: true,
                    enabled: Some(true),
                    focused: false,
                    actions: vec!["AXPress".into()],
                    frame: Some(Rect { x, y: 120.0, w: 80.0, h: 30.0 }),
                };
                let mut els = vec![
                    mk("e1", "Button", "Next", None, 20.0),
                    mk("e2", "TextArea", "Notes", Some(if p >= 1 { "updated" } else { "" }), 120.0),
                ];
                if p >= 1 {
                    els.push(mk("e3", "Button", "ConfirmNew", None, 260.0));
                }
                Ok(AppSnapshot {
                    bundle_id: bundle_id.into(),
                    frontmost_bundle_id: if p == 3 { "com.1password.app".into() } else { bundle_id.into() },
                    pid: 8080,
                    frontmost: true,
                    windows_total: 1,
                    windows: vec![crate::driver::WindowMeta {
                        index: 0,
                        title: "Form".into(),
                        frame: Some(Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }),
                    }],
                    elements: els,
                    has_secure_field: p == 2,
                    has_unverified_field: false,
                    truncated: false,
                    depth_truncated: false,
                    safety_scan_complete: true,
                })
            }
            fn window_state(&self, bundle_id: &str) -> Result<WindowState, crate::driver::DriverError> {
                Ok(WindowState {
                    frontmost_bundle_id: bundle_id.into(),
                    pid: 8080,
                    windows_total: 1,
                    active_window_frame: Some(Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }),
                })
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.actions.lock().unwrap().push(format!("{req:?}"));
                if self.phase.load(Ordering::SeqCst) == 0 {
                    self.phase.store(1, Ordering::SeqCst);
                }
                Ok(crate::driver::ActionResult { method: "ax".into() })
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_retstate_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let inspect_calls = Arc::new(AtomicUsize::new(0));
        let actions = Arc::new(Mutex::new(Vec::new()));
        let phase = Arc::new(AtomicU8::new(0));

        let mut gw = Gateway::new(
            Box::new(PostActionDriver {
                inspect_calls: inspect_calls.clone(),
                actions: actions.clone(),
                phase: phase.clone(),
            }),
            PolicySettings::default(),
            dir.clone(),
        );

        // 1. Initial inspect
        let init = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(init.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));
        assert_eq!(inspect_calls.load(Ordering::SeqCst), 1);

        // 2. Click e1 with return_state: "diff" -> captures post-action diff in 1 call and activates e3
        let clk1 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("return_state", JsonValue::String("diff".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(clk1.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(clk1.payload.get("state_mode").and_then(|v| v.as_str()), Some("diff"));
        assert_eq!(clk1.payload.get("snapshot_active").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(clk1.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(inspect_calls.load(Ordering::SeqCst), 2);
        let c1 = clk1.payload.get("content").and_then(|v| v.as_str()).unwrap();
        assert!(c1.contains("+ [e3] Button \"ConfirmNew\""), "got: {c1}");

        // 3. Immediately click newly appeared ref e3 with return_state: "full" -> 0 extra inspect_app calls before action!
        let clk2 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e3".into())),
                        ("return_state", JsonValue::String("full".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(clk2.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(clk2.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));
        assert_eq!(clk2.payload.get("snapshot_active").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(clk2.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(inspect_calls.load(Ordering::SeqCst), 3);
        assert!(actions.lock().unwrap().last().unwrap().contains("x: 300.0"));

        // 4. set_value and press_key also support return_state: "diff"
        let sv = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("abc".into())),
                        ("return_state", JsonValue::String("diff".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(sv.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(sv.payload.get("state_mode").and_then(|v| v.as_str()), Some("unchanged"));
        assert_eq!(sv.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(true));

        let pk = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("press_key".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("key", JsonValue::String("enter".into())),
                        ("return_state", JsonValue::String("diff".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(pk.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(pk.payload.get("snapshot_active").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(pk.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(true));

        // 5. run_actions with return_state: "diff" keeps refs live without duplicate keys
        let batch = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("return_state", JsonValue::String("diff".into())),
                        ("inspect_after", JsonValue::Bool(true)),
                        (
                            "steps",
                            JsonValue::Array(vec![
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("set_value".into())),
                                    ("ref", JsonValue::String("e2".into())),
                                    ("text", JsonValue::String("batch".into())),
                                ]),
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("click".into())),
                                    ("ref", JsonValue::String("e3".into())),
                                ]),
                            ]),
                        ),
                    ]),
                ),
            ]),
        );
        assert_eq!(batch.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(batch.payload.get("snapshot_active").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(batch.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(true));
        assert!(batch.payload.get("state_mode").and_then(|v| v.as_str()).is_some());
        let batch_json = batch.payload.to_json_string();
        assert_eq!(batch_json.matches("\"state_mode\":").count(), 1);
        assert_eq!(batch_json.matches("\"content\":").count(), 1);

        // 6. Default return_state: "none" on click invalidates refs and makes 0 post-action inspect calls
        let before_none = inspect_calls.load(Ordering::SeqCst);
        let clk_none = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(clk_none.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert!(clk_none.payload.get("content").is_none());
        assert_eq!(inspect_calls.load(Ordering::SeqCst), before_none);

        let stale = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(stale.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));

        // 7. Fail-closed redaction when post-action tree contains secure field or protected frontmost app
        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        phase.store(2, Ordering::SeqCst);
        let redacted = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("return_state", JsonValue::String("diff".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(redacted.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(redacted.payload.get("snapshot_active").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(redacted.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            redacted.payload.get("content").and_then(|v| v.as_str()),
            Some("[redacted: secure field detected]")
        );
        assert!(redacted.payload.get("redaction_reason").is_some());

        // Refs were cleared so clicking e1 now fails with stale_ref
        let stale_after_redact = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(stale_after_redact.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));

        // Protected frontmost app post-action also redacts and clears snapshot
        phase.store(1, Ordering::SeqCst);
        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        phase.store(3, Ordering::SeqCst);
        let prot_post = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("return_state", JsonValue::String("diff".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(prot_post.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(prot_post.payload.get("snapshot_active").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(prot_post.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            prot_post.payload.get("content").and_then(|v| v.as_str()),
            Some("[redacted: secure field detected]")
        );

        // Post-action inspect error keeps action ok: true and emits state_error
        phase.store(1, Ordering::SeqCst);
        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        phase.store(4, Ordering::SeqCst);
        let closed_win = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("return_state", JsonValue::String("diff".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(closed_win.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(closed_win.payload.get("snapshot_active").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(closed_win.payload.get("state_error").and_then(|v| v.as_str()), Some("window_not_found"));

        // Invalid return_state returns invalid_argument
        let bad_rs = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("return_state", JsonValue::String("bogus".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(bad_rs.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        // desktop activate_application & open_url also support return_state and keep refs live
        phase.store(1, Ordering::SeqCst);
        let act_ret = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("activate_application".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.apple.TextEdit".into())),
                        ("return_state", JsonValue::String("diff".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(act_ret.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(act_ret.payload.get("snapshot_active").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(act_ret.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(true));

        let url_ret = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("open_url".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.google.Chrome".into())),
                        ("url", JsonValue::String("https://example.com".into())),
                        ("return_state", JsonValue::String("diff".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(url_ret.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(url_ret.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));
        assert_eq!(url_ret.payload.get("snapshot_preserved").and_then(|v| v.as_bool()), Some(true));

        // computer(action="inspect") + window switch resets to full + config ax_diff=false/include_bounds=true
        let mut cfg = crate::config::AppConfig::default();
        cfg.snapshot.ax_diff = false;
        cfg.snapshot.include_bounds = true;
        let mut gw_cfg = Gateway::with_config(
            Box::new(MockDriver::new()),
            PolicySettings::default(),
            dir.clone(),
            cfg,
        );
        let c_insp1 = gw_cfg.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("inspect".into())),
                ("arguments", JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))])),
            ]),
        );
        assert_eq!(c_insp1.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));
        assert!(c_insp1.payload.get("content").and_then(|v| v.as_str()).unwrap().contains("@("));
        let c_insp2 = gw_cfg.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("inspect".into())),
                ("arguments", JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))])),
            ]),
        );
        assert_eq!(c_insp2.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));

        // With default ax_diff=true, changing window resets diff baseline to full
        let mut gw_win = Gateway::new(Box::new(MockDriver::new()), PolicySettings::default(), dir.clone());
        let w_none = gw_win.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        assert_eq!(w_none.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));
        let w_zero = gw_win.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("Calculator".into())),
                ("window", JsonValue::Number(0.0)),
            ]),
        );
        assert_eq!(w_zero.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));
        let w_zero_again = gw_win.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("Calculator".into())),
                ("window", JsonValue::Number(0.0)),
            ]),
        );
        assert_eq!(w_zero_again.payload.get("state_mode").and_then(|v| v.as_str()), Some("unchanged"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_mode_ax_skips_capture_and_visual_skips_ax() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct VerifyModeDriver {
            inspect_calls: Arc<AtomicUsize>,
            capture_calls: Arc<AtomicUsize>,
            acted: Arc<AtomicUsize>,
            ticks: Arc<AtomicUsize>,
        }
        impl OsDriver for VerifyModeDriver {
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
            fn open_url(&self, _: &str, _: &str) -> Result<(), crate::driver::DriverError> {
                Ok(())
            }
            fn supports_fast_point_verify(&self) -> bool {
                true
            }
            fn window_state(&self, bundle_id: &str) -> Result<WindowState, crate::driver::DriverError> {
                MockDriver::new().window_state(bundle_id)
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                me: i32,
                md: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                self.inspect_calls.fetch_add(1, Ordering::SeqCst);
                let mut snap = MockDriver::new().inspect_app(bundle_id, w, me, md)?;
                if self.acted.load(Ordering::SeqCst) > 0 && self.ticks.load(Ordering::SeqCst) >= 1 {
                    if let Some(el) = snap.elements.iter_mut().find(|e| e.ref_id == "e2") {
                        el.value = Some("ax-updated".into());
                    }
                }
                Ok(snap)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                self.capture_calls.fetch_add(1, Ordering::SeqCst);
                let mut cap = MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)?;
                let byte = if self.acted.load(Ordering::SeqCst) > 0 { "ff" } else { "10" };
                cap.pixel_digest = Some(byte.repeat(256));
                Ok(cap)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.acted.fetch_add(1, Ordering::SeqCst);
                MockDriver::new().perform_action(req)
            }
            fn tick(&self) {
                if self.acted.load(Ordering::SeqCst) > 0 {
                    self.ticks.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_vmode_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 1. verify_mode: "ax" skips capture_and_ocr completely while ax_changed exits early
        let inspect_ax = Arc::new(AtomicUsize::new(0));
        let capture_ax = Arc::new(AtomicUsize::new(0));
        let acted_ax = Arc::new(AtomicUsize::new(0));
        let ticks_ax = Arc::new(AtomicUsize::new(0));
        let mut gw_ax = Gateway::new(
            Box::new(VerifyModeDriver {
                inspect_calls: inspect_ax.clone(),
                capture_calls: capture_ax.clone(),
                acted: acted_ax,
                ticks: ticks_ax,
            }),
            PolicySettings::default(),
            dir.clone(),
        );
        gw_ax.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        let res_ax = gw_ax.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("verify_change", JsonValue::Bool(true)),
                        ("verify_mode", JsonValue::String("ax".into())),
                        ("settle_ms", JsonValue::Number(600.0)),
                        ("poll_interval_ms", JsonValue::Number(20.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(res_ax.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res_ax.payload.get("verified_change").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res_ax.payload.get("ax_changed").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res_ax.payload.get("visual_changed").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(capture_ax.load(Ordering::SeqCst), 0);

        // 2. verify_mode: "visual" skips inspect_app during verify polling
        let inspect_vis = Arc::new(AtomicUsize::new(0));
        let capture_vis = Arc::new(AtomicUsize::new(0));
        let acted_vis = Arc::new(AtomicUsize::new(0));
        let ticks_vis = Arc::new(AtomicUsize::new(0));
        let mut gw_vis = Gateway::new(
            Box::new(VerifyModeDriver {
                inspect_calls: inspect_vis.clone(),
                capture_calls: capture_vis.clone(),
                acted: acted_vis,
                ticks: ticks_vis,
            }),
            PolicySettings::default(),
            dir.clone(),
        );
        gw_vis.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        assert_eq!(inspect_vis.load(Ordering::SeqCst), 1);
        let res_vis = gw_vis.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("verify_change", JsonValue::Bool(true)),
                        ("verify_mode", JsonValue::String("visual".into())),
                        ("settle_ms", JsonValue::Number(400.0)),
                        ("poll_interval_ms", JsonValue::Number(20.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(res_vis.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res_vis.payload.get("verified_change").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res_vis.payload.get("visual_changed").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res_vis.payload.get("ax_changed").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(inspect_vis.load(Ordering::SeqCst), 1);
        assert!(capture_vis.load(Ordering::SeqCst) >= 2);

        // Invalid verify_mode rejected
        let bad_vm = gw_vis.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("verify_mode", JsonValue::String("magic".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(bad_vm.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_null_optional_fields_behave_like_omitted_keys() {
        let dir = std::env::temp_dir().join(format!("apexuse_gw_nulls_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut gw = Gateway::new(Box::new(MockDriver::new()), PolicySettings::default(), dir.clone());

        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );

        let click_ref_nulls = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e1".into())),
                        ("mark", JsonValue::Null),
                        ("x", JsonValue::Null),
                        ("y", JsonValue::Null),
                        ("target", JsonValue::Null),
                        ("window", JsonValue::Null),
                        ("return_state", JsonValue::Null),
                        ("verify_mode", JsonValue::Null),
                        ("keep_snapshot", JsonValue::Null),
                    ]),
                ),
            ]),
        );
        assert_eq!(click_ref_nulls.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let click_target_nulls = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("Calculator".into())),
                        (
                            "target",
                            JsonValue::object(vec![
                                ("query", JsonValue::String("7".into())),
                                ("role", JsonValue::Null),
                                ("fresh", JsonValue::Null),
                            ]),
                        ),
                        ("ref", JsonValue::Null),
                        ("mark", JsonValue::Null),
                        ("x", JsonValue::Null),
                        ("y", JsonValue::Null),
                    ]),
                ),
            ]),
        );
        assert_eq!(click_target_nulls.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        let set_val_nulls = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("hi".into())),
                        ("x", JsonValue::Null),
                        ("y", JsonValue::Null),
                        ("mark", JsonValue::Null),
                        ("target", JsonValue::Null),
                        ("keep_snapshot", JsonValue::Bool(true)),
                    ]),
                ),
            ]),
        );
        assert_eq!(set_val_nulls.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let sel_nulls = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("select_text".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("start", JsonValue::Null),
                        ("end", JsonValue::Null),
                        ("x", JsonValue::Null),
                        ("y", JsonValue::Null),
                        ("mark", JsonValue::Null),
                        ("target", JsonValue::Null),
                    ]),
                ),
            ]),
        );
        assert_eq!(sel_nulls.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );
        let scroll_unfiltered_nulls = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("scroll".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("direction", JsonValue::String("down".into())),
                        ("amount", JsonValue::Number(2.0)),
                        ("ref", JsonValue::Null),
                        ("mark", JsonValue::Null),
                        ("x", JsonValue::Null),
                        ("y", JsonValue::Null),
                        ("target", JsonValue::Null),
                    ]),
                ),
            ]),
        );
        assert_eq!(
            scroll_unfiltered_nulls.payload.get("ok").and_then(|v| v.as_bool()),
            Some(true)
        );

        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("Calculator".into())),
                ("query", JsonValue::String("7".into())),
            ]),
        );
        let scroll_filtered_nulls = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("scroll".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("direction", JsonValue::String("down".into())),
                        ("amount", JsonValue::Number(2.0)),
                        ("ref", JsonValue::Null),
                        ("mark", JsonValue::Null),
                        ("x", JsonValue::Null),
                        ("y", JsonValue::Null),
                    ]),
                ),
            ]),
        );
        assert_eq!(
            scroll_filtered_nulls.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_state")
        );

        let wait_null_query = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("wait_for".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("Calculator".into())),
                        ("query", JsonValue::Null),
                        ("text", JsonValue::String("Notes".into())),
                        ("role", JsonValue::Null),
                        ("timeout_ms", JsonValue::Number(500.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(wait_null_query.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // Non-null invalid types still fail closed
        let bad_ref = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::Number(123.0))])),
            ]),
        );
        assert_eq!(bad_ref.payload.get("code").and_then(|v| v.as_str()), Some("invalid_argument"));

        let bad_set_xy = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("hi".into())),
                        ("x", JsonValue::Number(100.0)),
                    ]),
                ),
            ]),
        );
        assert_eq!(
            bad_set_xy.payload.get("code").and_then(|v| v.as_str()),
            Some("target_identity_unverified")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_fails_closed_when_pending_gateway_action_missing_and_syncs_eviction() {
        use std::sync::{Arc, Mutex};
        struct RecordingDriver {
            log: Arc<Mutex<Vec<String>>>,
        }
        impl OsDriver for RecordingDriver {
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
                MockDriver::new().inspect_app(bundle_id, w, me, md)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(
                &self,
                req: &ActionRequest,
            ) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.log.lock().unwrap().push(format!("{req:?}"));
                Ok(crate::driver::ActionResult {
                    method: "mock_ok".into(),
                })
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_conf_sync_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let settings = PolicySettings {
            require_confirmation: true,
            ..PolicySettings::default()
        };
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut gw = Gateway::new(
            Box::new(RecordingDriver { log: log.clone() }),
            settings,
            dir.clone(),
        );
        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("Calculator".into()))]),
        );

        for (action_name, extra) in [
            ("set_value", vec![("text", JsonValue::String("abc".into()))]),
            ("select_text", vec![("start", JsonValue::Number(0.0)), ("end", JsonValue::Number(2.0))]),
        ] {
            let mut arg_entries = vec![("ref", JsonValue::String("e2".into()))];
            arg_entries.extend(extra);
            let pending = gw.dispatch(
                "computer",
                &JsonValue::object(vec![
                    ("action", JsonValue::String(action_name.into())),
                    ("arguments", JsonValue::object(arg_entries)),
                ]),
            );
            let tok = pending
                .payload
                .get("token")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();
            // Simulate missing pending_gateway_actions entry while ConfirmationStore holds token
            assert!(gw.pending_gateway_actions.remove(&tok).is_some());
            let conf = gw.dispatch(
                "computer",
                &JsonValue::object(vec![
                    ("action", JsonValue::String("confirm".into())),
                    ("arguments", JsonValue::object(vec![("token", JsonValue::String(tok))])),
                ]),
            );
            assert_eq!(conf.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
            assert_eq!(
                conf.payload.get("code").and_then(|v| v.as_str()),
                Some("invalid_confirmation")
            );
        }
        assert!(log.lock().unwrap().is_empty(), "must never downgrade to TypePoint");

        // >256 eviction stays synchronized across confirmations, pending_verify, and pending_gateway_actions
        let mut tokens = Vec::new();
        for i in 0..260 {
            let pending = gw.dispatch(
                "computer",
                &JsonValue::object(vec![
                    ("action", JsonValue::String("set_value".into())),
                    (
                        "arguments",
                        JsonValue::object(vec![
                            ("ref", JsonValue::String("e2".into())),
                            ("text", JsonValue::String(format!("val_{i}"))),
                        ]),
                    ),
                ]),
            );
            tokens.push(
                pending
                    .payload
                    .get("token")
                    .and_then(|v| v.as_str())
                    .unwrap()
                    .to_string(),
            );
        }
        assert_eq!(gw.pending_verify.len(), 256);
        assert_eq!(gw.pending_gateway_actions.len(), 256);

        let evicted = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                ("arguments", JsonValue::object(vec![("token", JsonValue::String(tokens[0].clone()))])),
            ]),
        );
        assert_eq!(
            evicted.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_confirmation")
        );
        assert!(log.lock().unwrap().is_empty());

        let newest = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                ("arguments", JsonValue::object(vec![("token", JsonValue::String(tokens[259].clone()))])),
            ]),
        );
        assert_eq!(newest.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        let entries = log.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].starts_with("SetValue {"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn semantic_matcher_disabled_exact_beats_enabled_partial_match() {
        let mut elements = HashMap::new();
        elements.insert(
            "e1".into(),
            UiElement {
                ref_id: "e1".into(),
                role: "Button".into(),
                label: "Save".into(),
                value: None,
                url: None,
                window: 0,
                depth: 1,
                interactive: true,
                enabled: Some(false),
                focused: false,
                actions: vec!["AXPress".into()],
                frame: Some(Rect { x: 10.0, y: 10.0, w: 60.0, h: 30.0 }),
            },
        );
        elements.insert(
            "e2".into(),
            UiElement {
                ref_id: "e2".into(),
                role: "Button".into(),
                label: "Save As...".into(),
                value: None,
                url: None,
                window: 0,
                depth: 1,
                interactive: true,
                enabled: Some(true),
                focused: false,
                actions: vec!["AXPress".into()],
                frame: Some(Rect { x: 80.0, y: 10.0, w: 80.0, h: 30.0 }),
            },
        );

        let spec = SemanticTargetSpec {
            query: "Save".into(),
            role: Some("Button".into()),
            fresh: false,
        };
        assert!(matches!(
            match_semantic_in_elements(&elements, &spec),
            Err(SemanticMatchError::Disabled)
        ));

        // Same-tier enabled exact match still wins over same-tier disabled exact match
        elements.insert(
            "e3".into(),
            UiElement {
                ref_id: "e3".into(),
                role: "Button".into(),
                label: "Save".into(),
                value: None,
                url: None,
                window: 0,
                depth: 1,
                interactive: true,
                enabled: Some(true),
                focused: false,
                actions: vec!["AXPress".into()],
                frame: Some(Rect { x: 170.0, y: 10.0, w: 60.0, h: 30.0 }),
            },
        );
        let matched = match_semantic_in_elements(&elements, &spec);
        assert!(matches!(matched, Ok(el) if el.ref_id == "e3"));
    }

    #[test]
    fn desktop_frontmost_change_forces_full_and_run_actions_tracks_step_filtered() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        struct DesktopSwitchDriver {
            phase: Arc<AtomicUsize>,
        }
        impl OsDriver for DesktopSwitchDriver {
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
                if bundle_id == "desktop" {
                    snap.frontmost_bundle_id = if self.phase.load(Ordering::SeqCst) == 0 {
                        "com.apple.calculator".into()
                    } else {
                        "com.apple.TextEdit".into()
                    };
                }
                Ok(snap)
            }
            fn capture_and_ocr(
                &self,
                bundle_id: &str,
                w: Option<i32>,
                ocr_mode: OcrMode,
                encode_image: bool,
                options: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, w, ocr_mode, encode_image, options)
            }
            fn render_som_image(
                &self,
                b: &str,
                m: &str,
                wf: &Rect,
                options: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(b, m, wf, options)
            }
            fn perform_action(&self, req: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                MockDriver::new().perform_action(req)
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_desk_filt_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let phase = Arc::new(AtomicUsize::new(0));
        let mut gw = Gateway::new(
            Box::new(DesktopSwitchDriver {
                phase: phase.clone(),
            }),
            PolicySettings::default(),
            dir.clone(),
        );

        let d1 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("desktop".into()))]),
        );
        assert_eq!(d1.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));

        // Switching frontmost_bundle_id under app="desktop" must force "full" even when pid (4242) matches
        phase.store(1, Ordering::SeqCst);
        let d2 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("desktop".into()))]),
        );
        assert_eq!(d2.payload.get("state_mode").and_then(|v| v.as_str()), Some("full"));

        // Same frontmost app again -> unchanged
        let d3 = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("desktop".into()))]),
        );
        assert_eq!(d3.payload.get("state_mode").and_then(|v| v.as_str()), Some("unchanged"));

        // Sub-issue B: run_actions step 1+ unscoped scroll fails closed when prior step left snapshot filtered
        let step_inspect_filtered_scroll = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("Calculator".into())),
                        (
                            "steps",
                            JsonValue::Array(vec![
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("inspect".into())),
                                    ("query", JsonValue::String("7".into())),
                                ]),
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("scroll".into())),
                                    ("direction", JsonValue::String("down".into())),
                                    ("amount", JsonValue::Number(3.0)),
                                ]),
                            ]),
                        ),
                    ]),
                ),
            ]),
        );
        assert_eq!(
            step_inspect_filtered_scroll.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_state")
        );

        // Initial filtered snapshot + sleep_ms step 0 + unscoped scroll step 1 also fails closed
        gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![
                ("app", JsonValue::String("Calculator".into())),
                ("query", JsonValue::String("7".into())),
            ]),
        );
        let sleep_then_scroll = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "steps",
                        JsonValue::Array(vec![
                            JsonValue::object(vec![
                                ("action", JsonValue::String("sleep_ms".into())),
                                ("ms", JsonValue::Number(10.0)),
                            ]),
                            JsonValue::object(vec![
                                ("action", JsonValue::String("scroll".into())),
                                ("direction", JsonValue::String("down".into())),
                                ("amount", JsonValue::Number(3.0)),
                            ]),
                        ]),
                    )]),
                ),
            ]),
        );
        assert_eq!(
            sleep_then_scroll.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_state")
        );

        // wait_for (AX match leaves snapshot filtered) + unscoped scroll step 1 also fails closed
        let wait_then_scroll = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("Calculator".into())),
                        (
                            "steps",
                            JsonValue::Array(vec![
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("wait_for".into())),
                                    ("query", JsonValue::String("Notes".into())),
                                    ("timeout_ms", JsonValue::Number(500.0)),
                                ]),
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("scroll".into())),
                                    ("direction", JsonValue::String("down".into())),
                                    ("amount", JsonValue::Number(3.0)),
                                ]),
                            ]),
                        ),
                    ]),
                ),
            ]),
        );
        assert_eq!(
            wait_then_scroll.payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_state")
        );

        // Unfiltered inspect clears step_filtered so subsequent unscoped scroll succeeds
        let reset_then_scroll = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("Calculator".into())),
                        (
                            "steps",
                            JsonValue::Array(vec![
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("inspect".into())),
                                    ("query", JsonValue::String("7".into())),
                                ]),
                                JsonValue::object(vec![("action", JsonValue::String("inspect".into()))]),
                                JsonValue::object(vec![
                                    ("action", JsonValue::String("scroll".into())),
                                    ("direction", JsonValue::String("down".into())),
                                    ("amount", JsonValue::Number(3.0)),
                                ]),
                            ]),
                        ),
                    ]),
                ),
            ]),
        );
        assert_eq!(reset_then_scroll.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(reset_then_scroll.payload.get("executed").and_then(|v| v.as_i64()), Some(3));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn doctor_includes_onboard_hint_only_when_macos_missing_permissions() {
        let dir = std::env::temp_dir().join(format!("apexuse_gw_doc_hint_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let gw_ax_missing = Gateway::new(
            Box::new(MockDriver::with_permissions("macos", false, true)),
            PolicySettings::default(),
            dir.clone(),
        );
        let doc_ax = gw_ax_missing.doctor();
        assert_eq!(doc_ax.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(doc_ax.get("os").and_then(|v| v.as_str()), Some("macos"));
        assert_eq!(doc_ax.get("accessibility").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(doc_ax.get("screen_recording").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            doc_ax.get("onboard_hint").and_then(|v| v.as_str()),
            Some("Run 'apexuse-mcp doctor --onboard' to open guided setup")
        );

        let gw_sr_missing = Gateway::new(
            Box::new(MockDriver::with_permissions("macos", true, false)),
            PolicySettings::default(),
            dir.clone(),
        );
        let doc_sr = gw_sr_missing.doctor();
        assert_eq!(
            doc_sr.get("onboard_hint").and_then(|v| v.as_str()),
            Some("Run 'apexuse-mcp doctor --onboard' to open guided setup")
        );

        let gw_macos_ok = Gateway::new(
            Box::new(MockDriver::with_permissions("macos", true, true)),
            PolicySettings::default(),
            dir.clone(),
        );
        assert!(gw_macos_ok.doctor().get("onboard_hint").is_none());

        let gw_linux_missing = Gateway::new(
            Box::new(MockDriver::with_permissions("linux", false, false)),
            PolicySettings::default(),
            dir.clone(),
        );
        assert!(gw_linux_missing.doctor().get("onboard_hint").is_none());

        let gw_mock_default = Gateway::new(
            Box::new(MockDriver::new()),
            PolicySettings::default(),
            dir.clone(),
        );
        assert!(gw_mock_default.doctor().get("onboard_hint").is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn gateway_onboard_handles_mock_granted_and_missing_states() {
        let dir = std::env::temp_dir().join(format!("apexuse_gw_onboard_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let gw_default = Gateway::new(
            Box::new(MockDriver::new()),
            PolicySettings::default(),
            dir.clone(),
        );
        let ob_default = gw_default.onboard(false, true);
        assert_eq!(ob_default.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ob_default.get("os").and_then(|v| v.as_str()), Some("mock"));
        assert_eq!(ob_default.get("shown").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(ob_default.get("already_granted").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ob_default.get("accessibility").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ob_default.get("screen_recording").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ob_default.get("completed").and_then(|v| v.as_bool()), Some(true));

        let gw_missing = Gateway::new(
            Box::new(MockDriver::with_permissions("macos", false, true)),
            PolicySettings::default(),
            dir.clone(),
        );
        let ob_missing = gw_missing.onboard(true, false);
        assert_eq!(ob_missing.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ob_missing.get("os").and_then(|v| v.as_str()), Some("macos"));
        assert_eq!(ob_missing.get("shown").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(ob_missing.get("already_granted").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(ob_missing.get("accessibility").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(ob_missing.get("screen_recording").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ob_missing.get("completed").and_then(|v| v.as_bool()), Some(false));

        let gw_non_macos = Gateway::new(
            Box::new(MockDriver::with_permissions("linux", true, true)),
            PolicySettings::default(),
            dir.clone(),
        );
        let ob_linux = gw_non_macos.onboard(false, true);
        assert_eq!(ob_linux.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ob_linux.get("os").and_then(|v| v.as_str()), Some("linux"));
        assert_eq!(ob_linux.get("shown").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            ob_linux.get("note").and_then(|v| v.as_str()),
            Some("macOS permission onboarding is only required on macOS")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_actions_mid_batch_error_reports_failed_step_and_executed_results() {
        let dir = std::env::temp_dir().join(format!("apexuse_gw_batch_partial_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut gw = Gateway::new(
            Box::new(MockDriver::new()),
            PolicySettings::default(),
            dir.clone(),
        );
        let insp = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        assert_eq!(insp.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        let res = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "steps",
                        JsonValue::Array(vec![
                            JsonValue::object(vec![
                                ("action", JsonValue::String("press_key".into())),
                                ("key", JsonValue::String("return".into())),
                            ]),
                            JsonValue::object(vec![
                                ("action", JsonValue::String("sleep_ms".into())),
                                ("ms", JsonValue::Number(10.0)),
                            ]),
                            JsonValue::object(vec![
                                ("action", JsonValue::String("click".into())),
                                ("ref", JsonValue::String("e999".into())),
                            ]),
                            JsonValue::object(vec![
                                ("action", JsonValue::String("press_key".into())),
                                ("key", JsonValue::String("tab".into())),
                            ]),
                        ]),
                    )]),
                ),
            ]),
        );

        assert_eq!(res.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(res.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));
        assert_eq!(res.payload.get("failed_step").and_then(|v| v.as_i64()), Some(2));
        assert_eq!(res.payload.get("executed").and_then(|v| v.as_i64()), Some(2));
        let results = res.payload.get("results").and_then(|v| v.as_array()).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].get("step").and_then(|v| v.as_i64()), Some(0));
        assert_eq!(results[0].get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(results[1].get("step").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(results[1].get("ok").and_then(|v| v.as_bool()), Some(true));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn direct_mode_rejects_stale_ref_when_element_identity_changes_before_click() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct StaleFastDriver {
            probe_mode: Arc<AtomicUsize>,
            acted: Arc<AtomicUsize>,
        }
        impl OsDriver for StaleFastDriver {
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
            fn supports_fast_point_verify(&self) -> bool {
                true
            }
            fn probe_point_safe(
                &self,
                _: &str,
                _x: f64,
                y: f64,
            ) -> Result<crate::driver::PointSafetyInfo, crate::driver::DriverError> {
                let mode = self.probe_mode.load(Ordering::SeqCst);
                let (role, label) = match mode {
                    0 => ("Button", "Save"),
                    1 => ("StaticText", "Save"), // Benign Chromium child hit with matching label
                    2 => ("Button", "Delete"),     // Mutated UI: wrong button shifted under ref
                    3 => ("Button", "Pay Now"),    // Editable field replaced by non-editable button
                    _ => ("TextField", "Notes"),
                };
                let (eff_role, eff_label) = if (y - 160.0).abs() <= 10.0 && mode == 0 {
                    ("TextField", "Notes")
                } else {
                    (role, label)
                };
                Ok(crate::driver::PointSafetyInfo {
                    safe: true,
                    role: Some(eff_role.into()),
                    label: Some(eff_label.into()),
                    frontmost_bundle_id: Some("com.apple.TextEdit".into()),
                })
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                _: Option<i32>,
                _: i32,
                _: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                Ok(AppSnapshot {
                    bundle_id: bundle_id.into(),
                    frontmost_bundle_id: bundle_id.into(),
                    pid: 42,
                    frontmost: true,
                    windows_total: 1,
                    windows: vec![crate::driver::WindowMeta {
                        index: 0,
                        title: "Doc".into(),
                        frame: Some(Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }),
                    }],
                    elements: vec![
                        UiElement {
                            ref_id: "e1".into(),
                            role: "Button".into(),
                            label: "Save".into(),
                            value: None,
                            url: None,
                            window: 0,
                            depth: 1,
                            interactive: true,
                            enabled: Some(true),
                            focused: false,
                            actions: vec!["AXPress".into()],
                            frame: Some(Rect { x: 80.0, y: 90.0, w: 80.0, h: 28.0 }),
                        },
                        UiElement {
                            ref_id: "e2".into(),
                            role: "TextField".into(),
                            label: "Notes".into(),
                            value: Some("".into()),
                            url: None,
                            window: 0,
                            depth: 1,
                            interactive: true,
                            enabled: Some(true),
                            focused: true,
                            actions: vec![],
                            frame: Some(Rect { x: 80.0, y: 146.0, w: 180.0, h: 28.0 }),
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
                window: Option<i32>,
                ocr: OcrMode,
                encode: bool,
                opts: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, window, ocr, encode, opts)
            }
            fn render_som_image(
                &self,
                img: &str,
                marks: &str,
                win: &Rect,
                opts: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(img, marks, win, opts)
            }
            fn perform_action(&self, _: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.acted.fetch_add(1, Ordering::SeqCst);
                Ok(crate::driver::ActionResult { method: "mock".into() })
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_stale_fast_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let probe_mode = Arc::new(AtomicUsize::new(0));
        let acted = Arc::new(AtomicUsize::new(0));
        let mut gw = Gateway::new(
            Box::new(StaleFastDriver {
                probe_mode: Arc::clone(&probe_mode),
                acted: Arc::clone(&acted),
            }),
            PolicySettings::default(),
            dir.clone(),
        );

        // 1. Matching element succeeds
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        let ok1 = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(ok1.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(acted.load(Ordering::SeqCst), 1);

        // 2. Child StaticText with matching label ("Save") also succeeds
        probe_mode.store(1, Ordering::SeqCst);
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        let ok_child = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(ok_child.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(acted.load(Ordering::SeqCst), 2);

        // 3. Layout shift changes Button "Save" -> Button "Delete" => fail closed with stale_ref
        probe_mode.store(2, Ordering::SeqCst);
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        let stale_btn = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(stale_btn.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(stale_btn.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));
        assert_eq!(acted.load(Ordering::SeqCst), 2);

        // 4. set_value fails closed when underlying field changed to Button
        probe_mode.store(3, Ordering::SeqCst);
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.apple.TextEdit".into()))]),
        );
        let stale_edit = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("set_value".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("ref", JsonValue::String("e2".into())),
                        ("text", JsonValue::String("hello".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(stale_edit.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(stale_edit.payload.get("code").and_then(|v| v.as_str()), Some("stale_ref"));
        assert_eq!(acted.load(Ordering::SeqCst), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn domain_policy_and_destructive_confirmation_enforce_boundaries() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct BrowserDriver {
            active_url: Arc<std::sync::Mutex<String>>,
            opened_urls: Arc<std::sync::Mutex<Vec<String>>>,
            acted: Arc<AtomicUsize>,
        }
        impl OsDriver for BrowserDriver {
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
            fn open_url(&self, _: &str, url: &str) -> Result<(), crate::driver::DriverError> {
                self.opened_urls.lock().unwrap().push(url.to_string());
                *self.active_url.lock().unwrap() = url.to_string();
                Ok(())
            }
            fn inspect_app(
                &self,
                bundle_id: &str,
                _: Option<i32>,
                _: i32,
                _: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                let url = self.active_url.lock().unwrap().clone();
                Ok(AppSnapshot {
                    bundle_id: bundle_id.into(),
                    frontmost_bundle_id: bundle_id.into(),
                    pid: 88,
                    frontmost: true,
                    windows_total: 1,
                    windows: vec![crate::driver::WindowMeta {
                        index: 0,
                        title: "Browser".into(),
                        frame: Some(Rect { x: 0.0, y: 0.0, w: 900.0, h: 700.0 }),
                    }],
                    elements: vec![
                        UiElement {
                            ref_id: "".into(),
                            role: "WebArea".into(),
                            label: "Page".into(),
                            value: None,
                            url: Some(url),
                            window: 0,
                            depth: 1,
                            interactive: false,
                            enabled: None,
                            focused: false,
                            actions: vec![],
                            frame: None,
                        },
                        UiElement {
                            ref_id: "e1".into(),
                            role: "Button".into(),
                            label: "Save".into(),
                            value: None,
                            url: None,
                            window: 0,
                            depth: 2,
                            interactive: true,
                            enabled: Some(true),
                            focused: false,
                            actions: vec!["AXPress".into()],
                            frame: Some(Rect { x: 40.0, y: 80.0, w: 80.0, h: 30.0 }),
                        },
                        UiElement {
                            ref_id: "e2".into(),
                            role: "Button".into(),
                            label: "Delete Account".into(),
                            value: None,
                            url: None,
                            window: 0,
                            depth: 2,
                            interactive: true,
                            enabled: Some(true),
                            focused: false,
                            actions: vec!["AXPress".into()],
                            frame: Some(Rect { x: 140.0, y: 80.0, w: 120.0, h: 30.0 }),
                        },
                        UiElement {
                            ref_id: "e3".into(),
                            role: "Link".into(),
                            label: "External Help".into(),
                            value: None,
                            url: Some("https://untrusted-link.example.org/docs".into()),
                            window: 0,
                            depth: 2,
                            interactive: true,
                            enabled: Some(true),
                            focused: false,
                            actions: vec!["AXPress".into()],
                            frame: Some(Rect { x: 40.0, y: 130.0, w: 120.0, h: 24.0 }),
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
                window: Option<i32>,
                ocr: OcrMode,
                encode: bool,
                opts: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, window, ocr, encode, opts)
            }
            fn render_som_image(
                &self,
                img: &str,
                marks: &str,
                win: &Rect,
                opts: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(img, marks, win, opts)
            }
            fn perform_action(&self, _: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.acted.fetch_add(1, Ordering::SeqCst);
                Ok(crate::driver::ActionResult { method: "mock".into() })
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_dom_pol_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let active_url = Arc::new(std::sync::Mutex::new("https://evil.docs.org/admin".to_string()));
        let opened_urls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let acted = Arc::new(AtomicUsize::new(0));
        let mut settings = PolicySettings::default();
        settings.allowed_domains = vec!["example.com".into(), "*.docs.org".into()];
        settings.blocked_domains = vec!["evil.docs.org".into()];
        settings.confirm_destructive = true;

        let mut gw = Gateway::new(
            Box::new(BrowserDriver {
                active_url: Arc::clone(&active_url),
                opened_urls: Arc::clone(&opened_urls),
                acted: Arc::clone(&acted),
            }),
            settings,
            dir.clone(),
        );

        // 1. Blocked URL in open_url fails closed before calling driver
        let bad_open = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("open_url".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.google.Chrome".into())),
                        ("url", JsonValue::String("https://evil.docs.org/phish".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(bad_open.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(bad_open.payload.get("code").and_then(|v| v.as_str()), Some("domain_blocked"));
        assert!(opened_urls.lock().unwrap().is_empty());

        // 2. Inspecting browser currently on blocked domain fails closed
        let bad_insp = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );
        assert_eq!(bad_insp.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(bad_insp.payload.get("code").and_then(|v| v.as_str()), Some("domain_blocked"));

        // 3. Open allowed domain -> succeeds, and external Link anchor on page does NOT block page inspect
        let ok_open = gw.dispatch(
            "desktop",
            &JsonValue::object(vec![
                ("action", JsonValue::String("open_url".into())),
                (
                    "arguments",
                    JsonValue::object(vec![
                        ("app", JsonValue::String("com.google.Chrome".into())),
                        ("url", JsonValue::String("https://guide.docs.org/start".into())),
                    ]),
                ),
            ]),
        );
        assert_eq!(ok_open.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        let ok_insp = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );
        assert_eq!(ok_insp.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // 4. Non-destructive click ("Save") executes directly
        let safe_click = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e1".into()))])),
            ]),
        );
        assert_eq!(safe_click.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(acted.load(Ordering::SeqCst), 1);

        // 5. Destructive click ("Delete Account") requires confirmation even in direct mode, then executes on confirm
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );
        let destr = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("click".into())),
                ("arguments", JsonValue::object(vec![("ref", JsonValue::String("e2".into()))])),
            ]),
        );
        assert_eq!(destr.payload.get("code").and_then(|v| v.as_str()), Some("confirmation_required"));
        assert_eq!(destr.payload.get("policy").and_then(|v| v.as_str()), Some("confirm"));
        let tok = destr.payload.get("token").and_then(|v| v.as_str()).unwrap().to_string();
        assert_eq!(acted.load(Ordering::SeqCst), 1);

        let conf = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("confirm".into())),
                ("arguments", JsonValue::object(vec![("token", JsonValue::String(tok))])),
            ]),
        );
        assert_eq!(conf.payload.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(acted.load(Ordering::SeqCst), 2);

        // 6. run_actions refuses destructive click step and reports confirmation_required with executed results
        let _ = gw.dispatch(
            "inspect_app",
            &JsonValue::object(vec![("app", JsonValue::String("com.google.Chrome".into()))]),
        );
        let batch_destr = gw.dispatch(
            "computer",
            &JsonValue::object(vec![
                ("action", JsonValue::String("run_actions".into())),
                (
                    "arguments",
                    JsonValue::object(vec![(
                        "steps",
                        JsonValue::Array(vec![
                            JsonValue::object(vec![
                                ("action", JsonValue::String("click".into())),
                                ("ref", JsonValue::String("e1".into())),
                            ]),
                            JsonValue::object(vec![
                                ("action", JsonValue::String("click".into())),
                                ("ref", JsonValue::String("e2".into())),
                            ]),
                        ]),
                    )]),
                ),
            ]),
        );
        assert_eq!(batch_destr.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(batch_destr.payload.get("code").and_then(|v| v.as_str()), Some("confirmation_required"));
        assert_eq!(batch_destr.payload.get("failed_step").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(batch_destr.payload.get("executed").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(acted.load(Ordering::SeqCst), 3);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn workflow_replay_enforces_preconditions_and_reports_step_failures() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct ReplayDriver {
            window_title: Arc<std::sync::Mutex<String>>,
            page_url: Arc<std::sync::Mutex<String>>,
            acted: Arc<AtomicUsize>,
        }
        impl OsDriver for ReplayDriver {
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
                _: Option<i32>,
                _: i32,
                _: i32,
            ) -> Result<AppSnapshot, crate::driver::DriverError> {
                Ok(AppSnapshot {
                    bundle_id: bundle_id.into(),
                    frontmost_bundle_id: bundle_id.into(),
                    pid: 99,
                    frontmost: true,
                    windows_total: 1,
                    windows: vec![crate::driver::WindowMeta {
                        index: 0,
                        title: self.window_title.lock().unwrap().clone(),
                        frame: Some(Rect { x: 0.0, y: 0.0, w: 700.0, h: 500.0 }),
                    }],
                    elements: vec![
                        UiElement {
                            ref_id: "".into(),
                            role: "WebArea".into(),
                            label: "Mail".into(),
                            value: None,
                            url: Some(self.page_url.lock().unwrap().clone()),
                            window: 0,
                            depth: 1,
                            interactive: false,
                            enabled: None,
                            focused: false,
                            actions: vec![],
                            frame: None,
                        },
                        UiElement {
                            ref_id: "e1".into(),
                            role: "Button".into(),
                            label: "Refresh".into(),
                            value: None,
                            url: None,
                            window: 0,
                            depth: 2,
                            interactive: true,
                            enabled: Some(true),
                            focused: false,
                            actions: vec!["AXPress".into()],
                            frame: Some(Rect { x: 30.0, y: 40.0, w: 80.0, h: 28.0 }),
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
                window: Option<i32>,
                ocr: OcrMode,
                encode: bool,
                opts: &ImageEncodeOptions,
            ) -> Result<crate::driver::CaptureResult, crate::driver::DriverError> {
                MockDriver::new().capture_and_ocr(bundle_id, window, ocr, encode, opts)
            }
            fn render_som_image(
                &self,
                img: &str,
                marks: &str,
                win: &Rect,
                opts: &ImageEncodeOptions,
            ) -> Result<String, crate::driver::DriverError> {
                MockDriver::new().render_som_image(img, marks, win, opts)
            }
            fn perform_action(&self, _: &ActionRequest) -> Result<crate::driver::ActionResult, crate::driver::DriverError> {
                self.acted.fetch_add(1, Ordering::SeqCst);
                Ok(crate::driver::ActionResult { method: "mock".into() })
            }
        }

        let dir = std::env::temp_dir().join(format!("apexuse_gw_replay_pre_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let window_title = Arc::new(std::sync::Mutex::new("Account Settings".to_string()));
        let page_url = Arc::new(std::sync::Mutex::new("https://mail.example.com/inbox".to_string()));
        let acted = Arc::new(AtomicUsize::new(0));
        let mut gw = Gateway::new(
            Box::new(ReplayDriver {
                window_title: Arc::clone(&window_title),
                page_url: Arc::clone(&page_url),
                acted: Arc::clone(&acted),
            }),
            PolicySettings::default(),
            dir.clone(),
        );

        let rec = gw.dispatch(
            "workflow_memory",
            &JsonValue::object(vec![
                ("action", JsonValue::String("record".into())),
                ("app", JsonValue::String("com.google.Chrome".into())),
                ("goal", JsonValue::String("Refresh and confirm".into())),
                ("expected_domain", JsonValue::String("mail.example.com".into())),
                ("expected_window_contains", JsonValue::String("Inbox".into())),
                (
                    "steps",
                    JsonValue::Array(vec![
                        JsonValue::object(vec![
                            ("action", JsonValue::String("click".into())),
                            ("target_role", JsonValue::String("Button".into())),
                            ("target_label", JsonValue::String("Refresh".into())),
                            ("note", JsonValue::String("Step 0".into())),
                        ]),
                        JsonValue::object(vec![
                            ("action", JsonValue::String("click".into())),
                            ("target_role", JsonValue::String("Button".into())),
                            ("target_label", JsonValue::String("MissingButton".into())),
                            ("note", JsonValue::String("Step 1".into())),
                        ]),
                    ]),
                ),
            ]),
        );
        assert_eq!(rec.payload.get("ok").and_then(|v| v.as_bool()), Some(true));

        // 1. Mismatched window title aborts BEFORE step 0
        let wrong_win = gw.dispatch(
            "workflow_memory",
            &JsonValue::object(vec![
                ("action", JsonValue::String("replay".into())),
                ("app", JsonValue::String("com.google.Chrome".into())),
                ("goal", JsonValue::String("Refresh".into())),
            ]),
        );
        assert_eq!(wrong_win.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(wrong_win.payload.get("code").and_then(|v| v.as_str()), Some("precondition_failed"));
        assert_eq!(acted.load(Ordering::SeqCst), 0);

        // 2. Window matches, domain mismatches -> aborts BEFORE step 0
        *window_title.lock().unwrap() = "Inbox (3 unread)".to_string();
        *page_url.lock().unwrap() = "https://other.org/home".to_string();
        let wrong_dom = gw.dispatch(
            "workflow_memory",
            &JsonValue::object(vec![
                ("action", JsonValue::String("replay".into())),
                ("app", JsonValue::String("com.google.Chrome".into())),
                ("goal", JsonValue::String("Refresh".into())),
            ]),
        );
        assert_eq!(wrong_dom.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(wrong_dom.payload.get("code").and_then(|v| v.as_str()), Some("precondition_failed"));
        assert_eq!(acted.load(Ordering::SeqCst), 0);

        // 3. Both match -> step 0 runs, step 1 misses -> returns failed_step, completed_steps, and partial results
        *page_url.lock().unwrap() = "https://mail.example.com/inbox".to_string();
        let partial = gw.dispatch(
            "workflow_memory",
            &JsonValue::object(vec![
                ("action", JsonValue::String("replay".into())),
                ("app", JsonValue::String("com.google.Chrome".into())),
                ("goal", JsonValue::String("Refresh".into())),
            ]),
        );
        assert_eq!(partial.payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(partial.payload.get("code").and_then(|v| v.as_str()), Some("replay_target_missing"));
        assert_eq!(partial.payload.get("failed_step").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(partial.payload.get("completed_steps").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(partial.payload.get("results").and_then(|v| v.as_array()).map(|a| a.len()), Some(1));
        assert_eq!(acted.load(Ordering::SeqCst), 1);

        let _ = fs::remove_dir_all(&dir);
    }
}
