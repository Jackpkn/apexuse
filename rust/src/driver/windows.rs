#![allow(dead_code)]

use super::{
    ActionRequest, ActionResult, AppSnapshot, CaptureResult, DoctorStatus, DriverError, OsDriver,
    Rect, RunningApp,
};

/// Windows OS backend placeholder implementing the shared `OsDriver` boundary.
/// Planned primitives: Microsoft UI Automation (`uiautomation`), `SendInput`, DXGI/GDI capture.
pub struct WindowsDriver;

impl OsDriver for WindowsDriver {
    fn doctor(&self) -> Result<DoctorStatus, DriverError> {
        Ok(DoctorStatus {
            os: "windows".into(),
            accessibility: false,
            screen_recording: false,
            note: "Windows UIA driver scaffold ready; compile on Windows with UIA backend enabled.".into(),
        })
    }

    fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows UIA driver is not implemented yet."))
    }

    fn activate_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows UIA driver is not implemented yet."))
    }

    fn launch_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows UIA driver is not implemented yet."))
    }

    fn inspect_app(&self, _bundle_id: &str, _window: Option<i32>, _max_elements: i32, _max_depth: i32) -> Result<AppSnapshot, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows UIA driver is not implemented yet."))
    }

    fn capture_and_ocr(&self, _bundle_id: &str, _window: Option<i32>, _run_ocr: bool) -> Result<CaptureResult, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows UIA driver is not implemented yet."))
    }

    fn render_som_png(&self, _png_b64: &str, _marks_json: &str, _win_frame: &Rect) -> Result<String, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows SoM renderer is not implemented yet."))
    }

    fn perform_action(&self, _req: &ActionRequest) -> Result<ActionResult, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows UIA driver is not implemented yet."))
    }
}
