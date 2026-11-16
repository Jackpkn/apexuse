pub mod linux;
pub mod macos;
pub mod windows;

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

impl AppSnapshot {
    pub fn render_outline(&self, query: Option<&str>) -> String {
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
                let title = self
                    .windows
                    .get(el.window)
                    .map(|w| w.title.as_str())
                    .unwrap_or("");
                if title.is_empty() {
                    lines.push(format!("── window {}:", el.window));
                } else {
                    lines.push(format!("── window {}: \"{}\"", el.window, title.replace('"', "'")));
                }
            }
            if let Some(ref term) = q {
                let in_label = el.label.to_ascii_lowercase().contains(term);
                let in_val = el
                    .value
                    .as_deref()
                    .map(|v| v.to_ascii_lowercase().contains(term))
                    .unwrap_or(false);
                let in_role = el.role.to_ascii_lowercase().contains(term);
                if !in_label && !in_val && !in_role {
                    continue;
                }
            }
            let indent = "  ".repeat(el.depth.min(6).max(1));
            let mut line = if el.ref_id.is_empty() {
                format!("{indent}{}", el.role)
            } else {
                format!("{indent}[{}] {}", el.ref_id, el.role)
            };
            if !el.label.is_empty() {
                line.push_str(&format!(" \"{}\"", el.label.replace('"', "'")));
            }
            if let Some(ref val) = el.value {
                line.push_str(&format!(" value=\"{}\"", val.replace('"', "'")));
            }
            if el.focused {
                line.push_str(" (focused)");
            }
            if el.enabled == Some(false) {
                line.push_str(" (disabled)");
            }
            if let Some(ref f) = el.frame {
                line.push_str(&format!(
                    " @({},{},{}x{})",
                    f.x.round() as i64,
                    f.y.round() as i64,
                    f.w.round() as i64,
                    f.h.round() as i64
                ));
            }
            lines.push(line);
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

#[derive(Debug, Clone)]
pub struct OcrBox {
    pub text: String,
    pub confidence: f64,
    pub frame: Rect,
}

#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub bundle_id: String,
    pub frontmost_bundle_id: String,
    pub pid: i64,
    pub window_id: u64,
    pub window_frame: Rect,
    pub image_width: usize,
    pub image_height: usize,
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
    },
}

#[derive(Debug, Clone)]
pub struct ActionResult {
    pub method: String,
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
    fn capture_and_ocr(
        &self,
        bundle_id: &str,
        window: Option<i32>,
        run_ocr: bool,
    ) -> Result<CaptureResult, DriverError>;
    fn render_som_png(
        &self,
        png_b64: &str,
        marks_json: &str,
        win_frame: &Rect,
    ) -> Result<String, DriverError>;
    fn perform_action(&self, req: &ActionRequest) -> Result<ActionResult, DriverError>;
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
