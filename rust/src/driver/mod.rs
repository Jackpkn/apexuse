pub mod common;
pub mod linux;
pub mod macos;
pub mod windows;

use std::collections::HashMap;

use crate::config::ImageFormat;
pub use common::is_editable_role;

#[derive(Debug, Clone, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone)]
pub struct DoctorStatus {
    pub os: String,
    pub accessibility: bool,
    pub screen_recording: bool,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct OnboardStatus {
    pub os: String,
    pub shown: bool,
    pub already_granted: bool,
    pub accessibility: bool,
    pub screen_recording: bool,
    pub completed: bool,
    pub host_app: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct RunningApp {
    pub bundle_id: String,
    pub pid: i64,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct WindowMeta {
    pub index: usize,
    pub title: String,
    pub frame: Option<Rect>,
}

#[derive(Debug, Clone)]
pub struct UiElement {
    pub ref_id: String,
    pub role: String,
    pub label: String,
    pub value: Option<String>,
    pub url: Option<String>,
    pub window: usize,
    pub depth: usize,
    pub interactive: bool,
    pub enabled: Option<bool>,
    pub focused: bool,
    pub actions: Vec<String>,
    pub frame: Option<Rect>,
}

#[derive(Debug, Clone)]
pub struct AppSnapshot {
    pub bundle_id: String,
    pub frontmost_bundle_id: String,
    pub pid: i64,
    pub frontmost: bool,
    pub windows_total: usize,
    pub windows: Vec<WindowMeta>,
    pub elements: Vec<UiElement>,
    pub has_secure_field: bool,
    pub has_unverified_field: bool,
    pub truncated: bool,
    pub depth_truncated: bool,
    pub safety_scan_complete: bool,
}

fn format_window_header(snap: &AppSnapshot, window: usize) -> String {
    let title = snap.windows.get(window).map(|w| w.title.as_str()).unwrap_or("");
    if title.is_empty() {
        format!("── window {window}:")
    } else {
        format!("── window {window}: \"{}\"", title.replace('"', "'"))
    }
}

fn format_element_line(el: &UiElement, include_bounds: bool) -> String {
    let mut line = if el.ref_id.is_empty() {
        el.role.clone()
    } else {
        format!("[{}] {}", el.ref_id, el.role)
    };
    if !el.label.is_empty() {
        line.push_str(&format!(" \"{}\"", el.label.replace('"', "'")));
    }
    if let Some(ref val) = el.value {
        line.push_str(&format!(" value=\"{}\"", val.replace('"', "'")));
    }
    if let Some(url) = el.url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let clean_url = url.replace(['\r', '\n'], " ").replace('"', "'");
        line.push_str(&format!(" URL={clean_url}"));
    }
    if el.focused {
        line.push_str(" (focused)");
    }
    if el.enabled == Some(false) {
        line.push_str(" (disabled)");
    }
    if include_bounds {
        if let Some(ref f) = el.frame {
            line.push_str(&format!(
                " @({},{},{}x{})",
                f.x.round() as i64,
                f.y.round() as i64,
                f.w.round() as i64,
                f.h.round() as i64
            ));
        }
    }
    line
}

impl AppSnapshot {
    pub fn render_outline(&self, query: Option<&str>) -> String {
        self.render_outline_with_options(query, true)
    }

    pub fn render_outline_with_options(&self, query: Option<&str>, include_bounds: bool) -> String {
        let q = query.map(|s| s.to_ascii_lowercase());
        let mut lines = Vec::new();
        lines.push(format!(
            "Application [{}] pid={} frontmost={} windows={}",
            self.bundle_id, self.pid, self.frontmost, self.windows_total
        ));
        let mut cur_win: Option<usize> = None;
        for el in &self.elements {
            if cur_win != Some(el.window) {
                cur_win = Some(el.window);
                lines.push(format_window_header(self, el.window));
            }
            if let Some(ref term) = q {
                let in_label = el.label.to_ascii_lowercase().contains(term);
                let in_val = el
                    .value
                    .as_deref()
                    .map(|v| v.to_ascii_lowercase().contains(term))
                    .unwrap_or(false);
                let in_role = el.role.to_ascii_lowercase().contains(term);
                let in_url = el
                    .url
                    .as_deref()
                    .map(|u| u.to_ascii_lowercase().contains(term))
                    .unwrap_or(false);
                if !in_label && !in_val && !in_role && !in_url {
                    continue;
                }
            }
            let indent = "  ".repeat(el.depth.min(6).max(1));
            lines.push(format!("{indent}{}", format_element_line(el, include_bounds)));
        }
        lines.join("\n")
    }

    pub fn comparable_text(&self) -> String {
        let mut out = format!(
            "Application [{}] pid={} frontmost=<state> windows={}",
            self.bundle_id, self.pid, self.windows_total
        );
        for el in &self.elements {
            out.push('\n');
            out.push_str(&format!(
                "[{}] {}:{}:{}",
                el.ref_id,
                el.role,
                el.label,
                el.value.as_deref().unwrap_or("")
            ));
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxDiffMode {
    Full,
    Diff,
    Unchanged,
}

impl AxDiffMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AxDiffMode::Full => "full",
            AxDiffMode::Diff => "diff",
            AxDiffMode::Unchanged => "unchanged",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxDiffResult {
    pub content: String,
    pub mode: AxDiffMode,
}

fn parse_numeric_ref(ref_id: &str) -> Option<usize> {
    ref_id.strip_prefix('e')?.parse::<usize>().ok()
}

fn max_numeric_ref(snap: &AppSnapshot) -> usize {
    snap.elements
        .iter()
        .filter_map(|el| parse_numeric_ref(&el.ref_id))
        .max()
        .unwrap_or(0)
}

fn frame_distance(a: &Option<Rect>, b: &Option<Rect>) -> f64 {
    match (a, b) {
        (Some(ra), Some(rb)) => {
            (ra.x - rb.x).abs() + (ra.y - rb.y).abs() + (ra.w - rb.w).abs() + (ra.h - rb.h).abs()
        }
        _ => f64::INFINITY,
    }
}

fn element_semantic_changed(a: &UiElement, b: &UiElement) -> bool {
    a.role != b.role
        || a.label != b.label
        || a.value != b.value
        || a.url != b.url
        || a.enabled != b.enabled
        || a.focused != b.focused
        || a.window != b.window
        || a.depth != b.depth
}

fn format_removed_refs(refs: &[&str]) -> String {
    let mut nums = Vec::new();
    let mut other = Vec::new();
    for &r in refs {
        if let Some(n) = parse_numeric_ref(r) {
            nums.push(n);
        } else if !r.is_empty() {
            other.push(r);
        }
    }
    nums.sort_unstable();
    nums.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < nums.len() {
        let start = nums[i];
        let mut end = start;
        while i + 1 < nums.len() && nums[i + 1] == end + 1 {
            i += 1;
            end = nums[i];
        }
        if start == end {
            parts.push(format!("e{start}"));
        } else {
            parts.push(format!("e{start}-e{end}"));
        }
        i += 1;
    }
    for r in other {
        parts.push(r.to_string());
    }
    parts.join(", ")
}

type MatchKey<'a> = (usize, &'a str, &'a str, usize, bool);

fn match_elements(prev: &AppSnapshot, next: &AppSnapshot) -> (Vec<Option<usize>>, Vec<bool>, usize) {
    let mut buckets: HashMap<MatchKey<'_>, (Vec<usize>, usize)> = HashMap::new();
    for (p_idx, p_el) in prev.elements.iter().enumerate() {
        let key = (
            p_el.window,
            p_el.role.as_str(),
            p_el.label.as_str(),
            p_el.depth,
            p_el.ref_id.is_empty(),
        );
        buckets.entry(key).or_insert_with(|| (Vec::new(), 0)).0.push(p_idx);
    }

    let mut prev_matched = vec![false; prev.elements.len()];
    let mut matches = vec![None; next.elements.len()];
    let mut matched_count = 0usize;

    for (n_idx, el) in next.elements.iter().enumerate() {
        let key = (
            el.window,
            el.role.as_str(),
            el.label.as_str(),
            el.depth,
            el.ref_id.is_empty(),
        );
        let Some((indices, head)) = buckets.get_mut(&key) else {
            continue;
        };
        while *head < indices.len() && prev_matched[indices[*head]] {
            *head += 1;
        }
        if *head >= indices.len() {
            continue;
        }
        let mut best_pos = *head;
        if el.frame.is_some() && *head + 1 < indices.len() {
            let mut best_dist = frame_distance(&el.frame, &prev.elements[indices[*head]].frame);
            if best_dist > 0.5 {
                for (pos, &p_idx) in indices.iter().enumerate().skip(*head + 1) {
                    if prev_matched[p_idx] {
                        continue;
                    }
                    let d = frame_distance(&el.frame, &prev.elements[p_idx].frame);
                    if d < best_dist {
                        best_dist = d;
                        best_pos = pos;
                        if best_dist <= 0.5 {
                            break;
                        }
                    }
                }
            }
        }
        let p_idx = indices[best_pos];
        prev_matched[p_idx] = true;
        matches[n_idx] = Some(p_idx);
        matched_count += 1;
        while *head < indices.len() && prev_matched[indices[*head]] {
            *head += 1;
        }
    }

    (matches, prev_matched, matched_count)
}

/// Reconciles ref IDs from `prev` into `next` and formats an incremental AX diff or full outline.
pub fn reconcile_and_diff_snapshot(
    prev: Option<&AppSnapshot>,
    next: &mut AppSnapshot,
    next_ref_id: &mut usize,
    include_bounds: bool,
    force_full: bool,
) -> AxDiffResult {
    let full_result = |snap: &AppSnapshot, ref_counter: &mut usize| {
        *ref_counter = max_numeric_ref(snap) + 1;
        AxDiffResult {
            content: snap.render_outline_with_options(None, include_bounds),
            mode: AxDiffMode::Full,
        }
    };

    let Some(p) = prev.filter(|p| {
        !force_full
            && p.bundle_id == next.bundle_id
            && p.pid == next.pid
            && (p.bundle_id != "desktop" || p.frontmost_bundle_id == next.frontmost_bundle_id)
            && !p.elements.is_empty()
    }) else {
        return full_result(next, next_ref_id);
    };

    let (matches, prev_matched, matched_count) = match_elements(p, next);
    let max_len = p.elements.len().max(next.elements.len());
    if matched_count == 0 || (max_len >= 5 && matched_count * 100 / max_len <= 20) {
        return full_result(next, next_ref_id);
    }

    let mut alloc_id = (*next_ref_id).max(max_numeric_ref(p) + 1).max(1);
    for (idx, el) in next.elements.iter_mut().enumerate() {
        if let Some(p_idx) = matches[idx] {
            el.ref_id = p.elements[p_idx].ref_id.clone();
        } else if !el.ref_id.is_empty() {
            el.ref_id = format!("e{alloc_id}");
            alloc_id += 1;
        }
    }
    *next_ref_id = alloc_id;

    let mut changed_count = 0usize;
    let mut added_count = 0usize;
    let mut entries: Vec<(char, usize)> = Vec::new();
    for (idx, el) in next.elements.iter().enumerate() {
        if let Some(p_idx) = matches[idx] {
            if element_semantic_changed(&p.elements[p_idx], el) {
                changed_count += 1;
                entries.push(('~', idx));
            }
        } else {
            added_count += 1;
            entries.push(('+', idx));
        }
    }

    let mut removed_refs: Vec<&str> = Vec::new();
    let mut removed_unreffed: Vec<&UiElement> = Vec::new();
    for (p_idx, p_el) in p.elements.iter().enumerate() {
        if !prev_matched[p_idx] {
            if !p_el.ref_id.is_empty() {
                removed_refs.push(p_el.ref_id.as_str());
            } else {
                removed_unreffed.push(p_el);
            }
        }
    }
    let removed_total = removed_refs.len() + removed_unreffed.len();

    if changed_count == 0 && added_count == 0 && removed_total == 0 {
        return AxDiffResult {
            content: format!(
                "Application [{}] pid={} frontmost={} windows={} (unchanged, {} elements)",
                next.bundle_id,
                next.pid,
                next.frontmost,
                next.windows_total,
                next.elements.len()
            ),
            mode: AxDiffMode::Unchanged,
        };
    }

    let mut lines = Vec::new();
    lines.push(format!(
        "Application [{}] pid={} frontmost={} windows={} (diff: {} changed, {} added, {} removed)",
        next.bundle_id,
        next.pid,
        next.frontmost,
        next.windows_total,
        changed_count,
        added_count,
        removed_total
    ));
    if !removed_refs.is_empty() {
        lines.push(format!("Removed refs: {}", format_removed_refs(&removed_refs)));
    }
    for r_el in removed_unreffed {
        lines.push(format!("- {}", format_element_line(r_el, false)));
    }

    let mut cur_win: Option<usize> = None;
    for (marker, idx) in entries {
        let el = &next.elements[idx];
        if cur_win != Some(el.window) {
            cur_win = Some(el.window);
            lines.push(format_window_header(next, el.window));
        }
        lines.push(format!("{marker} {}", format_element_line(el, include_bounds)));
    }

    AxDiffResult {
        content: lines.join("\n"),
        mode: AxDiffMode::Diff,
    }
}

#[derive(Debug, Clone)]
pub struct OcrBox {
    pub text: String,
    pub confidence: f64,
    pub frame: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrMode {
    Off,
    Fast,
    Accurate,
}

impl OcrMode {
    pub fn as_int(self) -> i32 {
        match self {
            OcrMode::Off => 0,
            OcrMode::Fast => 1,
            OcrMode::Accurate => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageEncodeOptions {
    pub format: ImageFormat,
    pub quality: u8,
    pub max_width: usize,
}

impl Default for ImageEncodeOptions {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            quality: 80,
            max_width: 1440,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowState {
    pub frontmost_bundle_id: String,
    pub pid: i64,
    pub windows_total: usize,
    pub active_window_frame: Option<Rect>,
}

#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub bundle_id: String,
    pub frontmost_bundle_id: String,
    pub pid: i64,
    pub window_id: u64,
    pub capture_id: Option<String>,
    pub pixel_digest: Option<String>,
    pub window_frame: Rect,
    pub image_width: usize,
    pub image_height: usize,
    pub image_format: ImageFormat,
    pub image_base64: String,
    pub png_base64: String,
    pub ocr_boxes: Vec<OcrBox>,
}

#[derive(Debug, Clone)]
pub enum ActionRequest {
    ClickPoint {
        bundle_id: String,
        x: f64,
        y: f64,
        prefer_ax: bool,
        click_count: u8,
        button: String,
    },
    HoverPoint {
        bundle_id: String,
        x: f64,
        y: f64,
    },
    DragPoints {
        bundle_id: String,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    TypePoint {
        bundle_id: String,
        x: f64,
        y: f64,
        text: String,
        mode: String,
        click_first: bool,
        key_delay_us: u64,
    },
    SetValue {
        bundle_id: String,
        x: f64,
        y: f64,
        text: String,
        key_delay_us: u64,
    },
    SelectText {
        bundle_id: String,
        x: f64,
        y: f64,
        start: Option<usize>,
        end: Option<usize>,
    },
    PressKey {
        bundle_id: String,
        key: String,
        modifiers: Vec<String>,
    },
    Scroll {
        bundle_id: String,
        direction: String,
        amount: i32,
        x: Option<f64>,
        y: Option<f64>,
    },
}

#[derive(Debug, Clone)]
pub struct ActionResult {
    pub method: String,
}

#[derive(Debug, Clone, Default)]
pub struct PointSafetyInfo {
    pub safe: bool,
    pub role: Option<String>,
    pub label: Option<String>,
    pub frontmost_bundle_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DriverError {
    pub code: String,
    pub message: String,
}

impl DriverError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub trait OsDriver: Send + Sync {
    fn doctor(&self) -> Result<DoctorStatus, DriverError>;
    fn onboard(&self, _force: bool, _auto_close: bool) -> Result<OnboardStatus, DriverError> {
        let os = if cfg!(target_os = "windows") {
            "windows".to_string()
        } else if cfg!(target_os = "linux") {
            "linux".to_string()
        } else {
            self.doctor()
                .map(|d| d.os)
                .unwrap_or_else(|_| std::env::consts::OS.to_string())
        };
        Ok(OnboardStatus {
            os,
            shown: false,
            already_granted: true,
            accessibility: true,
            screen_recording: true,
            completed: true,
            host_app: None,
            note: "macOS permission onboarding is only required on macOS".into(),
        })
    }
    fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError>;
    fn activate_app(&self, bundle_id: &str) -> Result<(), DriverError>;
    fn launch_app(&self, bundle_id: &str) -> Result<(), DriverError>;
    fn inspect_app(
        &self,
        bundle_id: &str,
        window: Option<i32>,
        max_elements: i32,
        max_depth: i32,
    ) -> Result<AppSnapshot, DriverError>;
    fn window_state(&self, _bundle_id: &str) -> Result<WindowState, DriverError> {
        Err(DriverError::new(
            "unsupported_on_platform",
            "window_state is not supported on this platform",
        ))
    }
    fn capture_and_ocr(
        &self,
        bundle_id: &str,
        window: Option<i32>,
        ocr_mode: OcrMode,
        encode_image: bool,
        options: &ImageEncodeOptions,
    ) -> Result<CaptureResult, DriverError>;
    fn render_som_image(
        &self,
        image_b64_or_cap_id: &str,
        marks_json: &str,
        win_frame: &Rect,
        options: &ImageEncodeOptions,
    ) -> Result<String, DriverError>;
    fn render_som_png(
        &self,
        png_b64_or_cap_id: &str,
        marks_json: &str,
        win_frame: &Rect,
    ) -> Result<String, DriverError> {
        let opts = ImageEncodeOptions::default();
        self.render_som_image(png_b64_or_cap_id, marks_json, win_frame, &opts)
    }
    fn perform_action(&self, req: &ActionRequest) -> Result<ActionResult, DriverError>;
    fn supports_fast_point_verify(&self) -> bool {
        false
    }
    fn verify_point_safe(&self, _bundle_id: &str, _x: f64, _y: f64) -> Result<bool, DriverError> {
        Ok(true)
    }
    fn probe_point_safe(&self, bundle_id: &str, x: f64, y: f64) -> Result<PointSafetyInfo, DriverError> {
        let safe = self.verify_point_safe(bundle_id, x, y)?;
        Ok(PointSafetyInfo {
            safe,
            ..Default::default()
        })
    }
    fn open_url(&self, _bundle_id: &str, _url: &str) -> Result<(), DriverError> {
        Err(DriverError::new(
            "unsupported_on_platform",
            "open_url is not supported on this platform",
        ))
    }
    fn tick(&self) {}
}

pub fn default_driver() -> Box<dyn OsDriver> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacosDriver)
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsDriver)
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        Box::new(linux::LinuxDriver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_editable_role_and_window_state() {
        assert!(common::is_editable_role("TextField"));
        assert!(common::is_editable_role("TextArea"));
        assert!(common::is_editable_role("SearchField"));
        assert!(common::is_editable_role("ComboBox"));
        assert!(!common::is_editable_role("Button"));
        assert!(!common::is_editable_role("StaticText"));

        let ws = WindowState {
            frontmost_bundle_id: "com.apple.finder".into(),
            pid: 100,
            windows_total: 2,
            active_window_frame: Some(Rect {
                x: 10.0,
                y: 20.0,
                w: 800.0,
                h: 600.0,
            }),
        };
        assert_eq!(ws.frontmost_bundle_id, "com.apple.finder");
        assert_eq!(ws.windows_total, 2);

        let set_act = ActionRequest::SetValue {
            bundle_id: "com.apple.finder".into(),
            x: 10.0,
            y: 20.0,
            text: "test".into(),
            key_delay_us: 500,
        };
        let sel_act = ActionRequest::SelectText {
            bundle_id: "com.apple.finder".into(),
            x: 10.0,
            y: 20.0,
            start: Some(0),
            end: Some(4),
        };
        match set_act {
            ActionRequest::SetValue { text, .. } => assert_eq!(text, "test"),
            _ => panic!("wrong variant"),
        }
        match sel_act {
            ActionRequest::SelectText { start, end, .. } => {
                assert_eq!(start, Some(0));
                assert_eq!(end, Some(4));
            }
            _ => panic!("wrong variant"),
        }
    }

    fn test_el(
        ref_id: &str,
        role: &str,
        label: &str,
        value: Option<&str>,
        url: Option<&str>,
        window: usize,
        depth: usize,
        frame: Option<Rect>,
    ) -> UiElement {
        UiElement {
            ref_id: ref_id.into(),
            role: role.into(),
            label: label.into(),
            value: value.map(str::to_string),
            url: url.map(str::to_string),
            window,
            depth,
            interactive: !ref_id.is_empty(),
            enabled: Some(true),
            focused: false,
            actions: vec![],
            frame,
        }
    }

    fn test_snap(bundle_id: &str, pid: i64, elements: Vec<UiElement>) -> AppSnapshot {
        AppSnapshot {
            bundle_id: bundle_id.into(),
            frontmost_bundle_id: bundle_id.into(),
            pid,
            frontmost: true,
            windows_total: 1,
            windows: vec![WindowMeta {
                index: 0,
                title: "Main".into(),
                frame: Some(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1000.0,
                    h: 800.0,
                }),
            }],
            elements,
            has_secure_field: false,
            has_unverified_field: false,
            truncated: false,
            depth_truncated: false,
            safety_scan_complete: true,
        }
    }

    #[test]
    fn test_render_outline_with_options_bounds_and_url() {
        let snap = test_snap(
            "com.google.Chrome",
            42,
            vec![test_el(
                "e1",
                "Link",
                "Docs",
                None,
                Some("https://example.com/docs"),
                0,
                2,
                Some(Rect {
                    x: 10.0,
                    y: 20.0,
                    w: 100.0,
                    h: 30.0,
                }),
            )],
        );

        let compact = snap.render_outline_with_options(None, false);
        assert!(!compact.contains("@("));
        assert!(compact.contains("URL=https://example.com/docs"));

        let full = snap.render_outline(None);
        assert!(full.contains("@(10,20,100x30)"));
        assert!(full.contains("URL=https://example.com/docs"));
    }

    #[test]
    fn test_reconcile_diff_initial_and_unchanged() {
        let mut s1 = test_snap(
            "com.google.Chrome",
            101,
            vec![
                test_el(
                    "e1",
                    "WebArea",
                    "Home",
                    None,
                    Some("https://example.com"),
                    0,
                    1,
                    Some(Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 }),
                ),
                test_el(
                    "e2",
                    "Button",
                    "Go",
                    None,
                    None,
                    0,
                    2,
                    Some(Rect { x: 20.0, y: 40.0, w: 60.0, h: 28.0 }),
                ),
            ],
        );
        let mut next_ref_id = 1;
        let first = reconcile_and_diff_snapshot(None, &mut s1, &mut next_ref_id, false, false);
        assert_eq!(first.mode, AxDiffMode::Full);
        assert_eq!(first.mode.as_str(), "full");
        assert_eq!(next_ref_id, 3);
        assert!(!first.content.contains("@("));

        // Identical tree with 2px frame jitter should still report Unchanged
        let mut s2 = test_snap(
            "com.google.Chrome",
            101,
            vec![
                test_el(
                    "e1",
                    "WebArea",
                    "Home",
                    None,
                    Some("https://example.com"),
                    0,
                    1,
                    Some(Rect { x: 2.0, y: 2.0, w: 800.0, h: 600.0 }),
                ),
                test_el(
                    "e2",
                    "Button",
                    "Go",
                    None,
                    None,
                    0,
                    2,
                    Some(Rect { x: 22.0, y: 42.0, w: 60.0, h: 28.0 }),
                ),
            ],
        );
        let second = reconcile_and_diff_snapshot(Some(&s1), &mut s2, &mut next_ref_id, false, false);
        assert_eq!(second.mode, AxDiffMode::Unchanged);
        assert_eq!(second.mode.as_str(), "unchanged");
        assert!(!second.content.contains('\n'));
        assert_eq!(next_ref_id, 3);
    }

    #[test]
    fn test_reconcile_diff_small_edits_preserves_refs_and_ranges() {
        let prev = test_snap(
            "com.google.Chrome",
            202,
            vec![
                test_el("e1", "Button", "Back", None, None, 0, 1, Some(Rect { x: 10.0, y: 10.0, w: 40.0, h: 24.0 })),
                test_el("e2", "StaticText", "Row 1", None, None, 0, 2, Some(Rect { x: 10.0, y: 50.0, w: 80.0, h: 20.0 })),
                test_el("e3", "StaticText", "Row 2", None, None, 0, 2, Some(Rect { x: 10.0, y: 75.0, w: 80.0, h: 20.0 })),
                test_el("e4", "StaticText", "Row 3", None, None, 0, 2, Some(Rect { x: 10.0, y: 100.0, w: 80.0, h: 20.0 })),
                test_el("e5", "Button", "Submit", None, None, 0, 1, Some(Rect { x: 10.0, y: 140.0, w: 80.0, h: 30.0 })),
                test_el("e6", "Button", "Delete", None, None, 0, 1, Some(Rect { x: 100.0, y: 140.0, w: 60.0, h: 30.0 })),
                test_el("e7", "Button", "Delete", None, None, 0, 1, Some(Rect { x: 220.0, y: 140.0, w: 60.0, h: 30.0 })),
                test_el("e8", "TextField", "Search", None, None, 0, 1, Some(Rect { x: 10.0, y: 180.0, w: 200.0, h: 28.0 })),
                test_el("e9", "WebArea", "Docs", None, Some("https://a.test"), 0, 1, Some(Rect { x: 0.0, y: 220.0, w: 800.0, h: 400.0 })),
            ],
        );

        let mut search_el = test_el(
            "e4",
            "TextField",
            "Search",
            Some("rust"),
            None,
            0,
            1,
            Some(Rect { x: 10.0, y: 180.0, w: 200.0, h: 28.0 }),
        );
        search_el.focused = true;

        let mut next = test_snap(
            "com.google.Chrome",
            202,
            vec![
                test_el("e1", "Button", "Back", None, None, 0, 1, Some(Rect { x: 10.0, y: 10.0, w: 40.0, h: 24.0 })),
                test_el("e2", "Button", "Submit", None, None, 0, 1, Some(Rect { x: 10.0, y: 140.0, w: 80.0, h: 30.0 })),
                // Remaining "Delete" sits at x=220 -> should match e7, not e6!
                test_el("e3", "Button", "Delete", None, None, 0, 1, Some(Rect { x: 220.0, y: 140.0, w: 60.0, h: 30.0 })),
                search_el,
                test_el("e5", "WebArea", "Docs", None, Some("https://b.test"), 0, 1, Some(Rect { x: 0.0, y: 220.0, w: 800.0, h: 400.0 })),
                test_el("e6", "Button", "Export", None, None, 0, 1, Some(Rect { x: 300.0, y: 140.0, w: 70.0, h: 30.0 })),
                // Empty ref_id (e.g. non-interactive / redacted marker) must NOT be assigned e<N>
                test_el("", "StaticText", "Footer Note", None, None, 0, 1, None),
            ],
        );

        let mut next_ref_id = 10;
        let diff = reconcile_and_diff_snapshot(Some(&prev), &mut next, &mut next_ref_id, false, false);
        assert_eq!(diff.mode, AxDiffMode::Diff);
        assert_eq!(diff.mode.as_str(), "diff");
        assert_eq!(next.elements[0].ref_id, "e1");
        assert_eq!(next.elements[1].ref_id, "e5");
        assert_eq!(next.elements[2].ref_id, "e7");
        assert_eq!(next.elements[3].ref_id, "e8");
        assert_eq!(next.elements[4].ref_id, "e9");
        assert_eq!(next.elements[5].ref_id, "e10");
        assert_eq!(next.elements[6].ref_id, "");
        assert_eq!(next_ref_id, 11);

        assert!(diff.content.contains("Removed refs: e2-e4, e6"));
        assert!(diff.content.contains("~ [e8] TextField \"Search\" value=\"rust\" (focused)"));
        assert!(diff.content.contains("~ [e9] WebArea \"Docs\" URL=https://b.test"));
        assert!(diff.content.contains("+ [e10] Button \"Export\""));
        assert!(diff.content.contains("+ StaticText \"Footer Note\""));
        assert!(!diff.content.contains("@("));

        // Follow-up snapshot adds another button; must allocate e11 (never collide with removed e2-e4/e6)
        let mut next2 = next.clone();
        next2.elements.push(test_el(
            "e99",
            "Button",
            "Share",
            None,
            None,
            0,
            1,
            Some(Rect { x: 380.0, y: 140.0, w: 70.0, h: 30.0 }),
        ));
        let diff2 = reconcile_and_diff_snapshot(Some(&next), &mut next2, &mut next_ref_id, true, false);
        assert_eq!(diff2.mode, AxDiffMode::Diff);
        assert_eq!(next2.elements.last().unwrap().ref_id, "e11");
        assert_eq!(next_ref_id, 12);
        assert!(diff2.content.contains("+ [e11] Button \"Share\" @(380,140,70x30)"));
    }

    #[test]
    fn test_reconcile_diff_high_churn_and_incompatible_reset_to_full() {
        let prev_els: Vec<UiElement> = (1..=10)
            .map(|i| test_el(&format!("e{i}"), "Button", &format!("Old {i}"), None, None, 0, 1, None))
            .collect();
        let prev = test_snap("com.google.Chrome", 303, prev_els);

        // 9 of 10 replaced -> 10% matched (<= 20%, >80% changed) => Full
        let mut next_els: Vec<UiElement> = vec![
            test_el("e1", "Button", "Old 1", None, None, 0, 1, None),
        ];
        for i in 2..=10 {
            next_els.push(test_el(&format!("e{i}"), "Button", &format!("New {i}"), None, None, 0, 1, None));
        }
        let mut next = test_snap("com.google.Chrome", 303, next_els);
        let mut next_ref_id = 50;
        let res = reconcile_and_diff_snapshot(Some(&prev), &mut next, &mut next_ref_id, false, false);
        assert_eq!(res.mode, AxDiffMode::Full);
        assert_eq!(next.elements[0].ref_id, "e1");
        assert_eq!(next.elements[9].ref_id, "e10");
        assert_eq!(next_ref_id, 11);

        // Force full or mismatched pid also resets to Full
        let mut diff_pid = test_snap(
            "com.google.Chrome",
            999,
            vec![test_el("e1", "Button", "Old 1", None, None, 0, 1, None)],
        );
        let res_pid = reconcile_and_diff_snapshot(Some(&prev), &mut diff_pid, &mut next_ref_id, false, false);
        assert_eq!(res_pid.mode, AxDiffMode::Full);
        assert_eq!(next_ref_id, 2);

        let mut forced = prev.clone();
        let res_forced = reconcile_and_diff_snapshot(Some(&prev), &mut forced, &mut next_ref_id, false, true);
        assert_eq!(res_forced.mode, AxDiffMode::Full);
        assert_eq!(next_ref_id, 11);
    }

    #[test]
    fn default_onboard_returns_non_macos_completed_without_ui() {
        struct StubNonMacosDriver;
        impl OsDriver for StubNonMacosDriver {
            fn doctor(&self) -> Result<DoctorStatus, DriverError> {
                Ok(DoctorStatus {
                    os: "linux".into(),
                    accessibility: true,
                    screen_recording: true,
                    note: "stub".into(),
                })
            }
            fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError> {
                Ok(vec![])
            }
            fn activate_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
                Ok(())
            }
            fn launch_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
                Ok(())
            }
            fn inspect_app(&self, _b: &str, _w: Option<i32>, _me: i32, _md: i32) -> Result<AppSnapshot, DriverError> {
                unreachable!()
            }
            fn capture_and_ocr(&self, _b: &str, _w: Option<i32>, _om: OcrMode, _ei: bool, _o: &ImageEncodeOptions) -> Result<CaptureResult, DriverError> {
                unreachable!()
            }
            fn render_som_image(&self, _i: &str, _m: &str, _wf: &Rect, _o: &ImageEncodeOptions) -> Result<String, DriverError> {
                unreachable!()
            }
            fn perform_action(&self, _req: &ActionRequest) -> Result<ActionResult, DriverError> {
                unreachable!()
            }
        }

        let ob = StubNonMacosDriver.onboard(false, true).unwrap();
        assert!(!ob.shown);
        assert!(ob.accessibility);
        assert!(ob.screen_recording);
        assert!(ob.completed);
        assert!(ob.host_app.is_none());
        assert_eq!(ob.note, "macOS permission onboarding is only required on macOS");
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        assert_eq!(ob.os, "linux");
    }
}
