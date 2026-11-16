#![allow(dead_code)]

use super::{
    ActionRequest, ActionResult, AppSnapshot, CaptureResult, DoctorStatus, DriverError, OsDriver,
    Rect, RunningApp,
};

/// Linux OS backend placeholder implementing the shared `OsDriver` boundary.
/// Planned primitives: AT-SPI2 over D-Bus (`atspi`), X11/Wayland portal capture & input.
pub struct LinuxDriver;

impl OsDriver for LinuxDriver {
    fn doctor(&self) -> Result<DoctorStatus, DriverError> {
        Ok(DoctorStatus {
            os: "linux".into(),
            accessibility: false,
            screen_recording: false,
            note: "Linux AT-SPI2 driver scaffold ready; compile on Linux with AT-SPI2 backend enabled.".into(),
        })
    }

    fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux AT-SPI2 driver is not implemented yet."))
    }

    fn activate_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux AT-SPI2 driver is not implemented yet."))
    }

    fn launch_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux AT-SPI2 driver is not implemented yet."))
    }

    fn inspect_app(&self, _bundle_id: &str, _window: Option<i32>, _max_elements: i32, _max_depth: i32) -> Result<AppSnapshot, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux AT-SPI2 driver is not implemented yet."))
    }

    fn capture_and_ocr(&self, _bundle_id: &str, _window: Option<i32>, _run_ocr: bool) -> Result<CaptureResult, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux AT-SPI2 driver is not implemented yet."))
    }

    fn render_som_png(&self, _png_b64: &str, _marks_json: &str, _win_frame: &Rect) -> Result<String, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux SoM renderer is not implemented yet."))
    }

    fn perform_action(&self, _req: &ActionRequest) -> Result<ActionResult, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux AT-SPI2 driver is not implemented yet."))
    }
}
