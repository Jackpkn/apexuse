#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use super::{
    ActionRequest, ActionResult, AppSnapshot, CaptureResult, DoctorStatus, DriverError, OcrBox,
    OcrMode, OsDriver, Rect, RunningApp, UiElement, WindowMeta,
};
use crate::json::{parse_json, JsonValue};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int};

#[cfg(target_os = "macos")]
extern "C" {
    fn apexuse_free_cstr(ptr: *mut c_char);
    fn apexuse_macos_doctor() -> *mut c_char;
    fn apexuse_macos_list_apps() -> *mut c_char;
    fn apexuse_macos_activate_app(bundle_id: *const c_char) -> *mut c_char;
    fn apexuse_macos_launch_app(bundle_id: *const c_char) -> *mut c_char;
    fn apexuse_macos_inspect_app(
        bundle_id: *const c_char,
        window_index: c_int,
        max_elements: c_int,
        max_depth: c_int,
    ) -> *mut c_char;
    fn apexuse_macos_capture_and_ocr(
        bundle_id: *const c_char,
        window_index: c_int,
        ocr_mode: c_int,
        encode_png: c_int,
    ) -> *mut c_char;
    fn apexuse_macos_render_som(
        png_b64_or_cap_id: *const c_char,
        marks_json: *const c_char,
        win_x: c_double,
        win_y: c_double,
        win_w: c_double,
        win_h: c_double,
    ) -> *mut c_char;
    fn apexuse_macos_perform_action(req_json: *const c_char) -> *mut c_char;
    fn apexuse_macos_check_point(
        bundle_id: *const c_char,
        x: c_double,
        y: c_double,
    ) -> *mut c_char;
    fn apexuse_macos_open_url(bundle_id: *const c_char, url: *const c_char) -> *mut c_char;
    fn apexuse_macos_tick();
}

pub struct MacosDriver;

#[cfg(target_os = "macos")]
fn take_json(ptr: *mut c_char) -> Result<JsonValue, DriverError> {
    if ptr.is_null() {
        return Err(DriverError::new("native_null", "Native bridge returned null"));
    }
    let text = unsafe {
        let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        apexuse_free_cstr(ptr);
        s
    };
    let val = parse_json(&text).map_err(|e| DriverError::new("native_json_error", e))?;
    if val.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let code = val
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("upstream_error");
        let msg = val
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Native call failed");
        return Err(DriverError::new(code, msg));
    }
    Ok(val)
}

#[cfg(target_os = "macos")]
fn to_cstr(s: &str) -> Result<CString, DriverError> {
    CString::new(s).map_err(|_| DriverError::new("invalid_argument", "Unexpected NUL byte"))
}

fn parse_rect(v: &JsonValue) -> Option<Rect> {
    Some(Rect {
        x: v.get("x")?.as_f64()?,
        y: v.get("y")?.as_f64()?,
        w: v.get("w")?.as_f64()?,
        h: v.get("h")?.as_f64()?,
    })
}

#[cfg(target_os = "macos")]
impl OsDriver for MacosDriver {
    fn doctor(&self) -> Result<DoctorStatus, DriverError> {
        let val = take_json(unsafe { apexuse_macos_doctor() })?;
        Ok(DoctorStatus {
            os: "macos".into(),
            accessibility: val
                .get("accessibility")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            screen_recording: val
                .get("screen_recording")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            note: "macOS AXUIElement + ScreenCaptureKit + Apple Vision driver active".into(),
        })
    }

    fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError> {
        let val = take_json(unsafe { apexuse_macos_list_apps() })?;
        let arr = val
            .get("apps")
            .and_then(|v| v.as_array())
            .ok_or_else(|| DriverError::new("invalid_upstream_response", "Missing apps array"))?;
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let bundle_id = item
                .get("bundle_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let pid = item.get("pid").and_then(|v| v.as_i64()).unwrap_or(0);
            let active = item.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
            if !bundle_id.is_empty() && pid > 0 {
                out.push(RunningApp {
                    bundle_id,
                    pid,
                    active,
                });
            }
        }
        Ok(out)
    }

    fn activate_app(&self, bundle_id: &str) -> Result<(), DriverError> {
        let c = to_cstr(bundle_id)?;
        take_json(unsafe { apexuse_macos_activate_app(c.as_ptr()) })?;
        Ok(())
    }

    fn launch_app(&self, bundle_id: &str) -> Result<(), DriverError> {
        let c = to_cstr(bundle_id)?;
        take_json(unsafe { apexuse_macos_launch_app(c.as_ptr()) })?;
        Ok(())
    }

    fn inspect_app(
        &self,
        bundle_id: &str,
        window: Option<i32>,
        max_elements: i32,
        max_depth: i32,
    ) -> Result<AppSnapshot, DriverError> {
        let c = to_cstr(bundle_id)?;
        let win_idx = window.unwrap_or(-1);
        let val = take_json(unsafe {
            apexuse_macos_inspect_app(c.as_ptr(), win_idx, max_elements, max_depth)
        })?;

        let mut windows = Vec::new();
        if let Some(w_arr) = val.get("windows").and_then(|v| v.as_array()) {
            for w in w_arr {
                windows.push(WindowMeta {
                    index: w.get("index").and_then(|v| v.as_i64()).unwrap_or(0) as usize,
                    title: w
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    frame: w.get("frame").and_then(parse_rect),
                });
            }
        }

        let mut elements = Vec::new();
        if let Some(e_arr) = val.get("elements").and_then(|v| v.as_array()) {
            for e in e_arr {
                let actions = e
                    .get("actions")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                elements.push(UiElement {
                    ref_id: e
                        .get("ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    role: e
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown")
                        .to_string(),
                    label: e
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    value: e.get("value").and_then(|v| v.as_str()).map(str::to_string),
                    window: e.get("window").and_then(|v| v.as_i64()).unwrap_or(0) as usize,
                    depth: e.get("depth").and_then(|v| v.as_i64()).unwrap_or(0) as usize,
                    interactive: e
                        .get("interactive")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    enabled: e.get("enabled").and_then(|v| v.as_bool()),
                    focused: e.get("focused").and_then(|v| v.as_bool()).unwrap_or(false),
                    actions,
                    frame: e.get("frame").and_then(parse_rect),
                });
            }
        }

        let bundle_out = val
            .get("bundle_id")
            .and_then(|v| v.as_str())
            .unwrap_or(bundle_id)
            .to_string();
        let front_out = val
            .get("frontmost_bundle_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&bundle_out)
            .to_string();

        Ok(AppSnapshot {
            bundle_id: bundle_out,
            frontmost_bundle_id: front_out,
            pid: val.get("pid").and_then(|v| v.as_i64()).unwrap_or(0),
            frontmost: val
                .get("frontmost")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            windows_total: val
                .get("windows_total")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as usize,
            windows,
            elements,
            has_secure_field: val
                .get("has_secure_field")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            has_unverified_field: val
                .get("has_unverified_field")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            truncated: val
                .get("truncated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            depth_truncated: val
                .get("depth_truncated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            safety_scan_complete: val
                .get("safety_scan_complete")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }

    fn capture_and_ocr(
        &self,
        bundle_id: &str,
        window: Option<i32>,
        ocr_mode: OcrMode,
        encode_png: bool,
    ) -> Result<CaptureResult, DriverError> {
        let c = to_cstr(bundle_id)?;
        let win_idx = window.unwrap_or(-1);
        let val = take_json(unsafe {
            apexuse_macos_capture_and_ocr(
                c.as_ptr(),
                win_idx,
                ocr_mode.as_int(),
                if encode_png { 1 } else { 0 },
            )
        })?;
        let wf = val
            .get("window_frame")
            .and_then(parse_rect)
            .ok_or_else(|| DriverError::new("invalid_upstream_response", "Missing window_frame"))?;
        let mut ocr_boxes = Vec::new();
        if let Some(arr) = val.get("ocr_boxes").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(rect) = parse_rect(item) {
                    ocr_boxes.push(OcrBox {
                        text: item
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        confidence: item
                            .get("confidence")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0),
                        frame: rect,
                    });
                }
            }
        }
        let cap_bundle = val
            .get("bundle_id")
            .and_then(|v| v.as_str())
            .unwrap_or(bundle_id)
            .to_string();
        let cap_front = val
            .get("frontmost_bundle_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&cap_bundle)
            .to_string();
        let capture_id = val
            .get("capture_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let pixel_digest = val
            .get("pixel_digest")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(CaptureResult {
            bundle_id: cap_bundle,
            frontmost_bundle_id: cap_front,
            pid: val.get("pid").and_then(|v| v.as_i64()).unwrap_or(0),
            window_id: val.get("window_id").and_then(|v| v.as_i64()).unwrap_or(0) as u64,
            capture_id,
            pixel_digest,
            window_frame: wf,
            image_width: val.get("image_width").and_then(|v| v.as_i64()).unwrap_or(0) as usize,
            image_height: val.get("image_height").and_then(|v| v.as_i64()).unwrap_or(0) as usize,
            png_base64: val
                .get("png_base64")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            ocr_boxes,
        })
    }

    fn render_som_png(
        &self,
        png_b64: &str,
        marks_json: &str,
        win_frame: &Rect,
    ) -> Result<String, DriverError> {
        let cb64 = to_cstr(png_b64)?;
        let cmarks = to_cstr(marks_json)?;
        let val = take_json(unsafe {
            apexuse_macos_render_som(
                cb64.as_ptr(),
                cmarks.as_ptr(),
                win_frame.x,
                win_frame.y,
                win_frame.w,
                win_frame.h,
            )
        })?;
        val.get("png_base64")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| DriverError::new("render_failed", "Missing annotated png_base64"))
    }

    fn perform_action(&self, req: &ActionRequest) -> Result<ActionResult, DriverError> {
        let payload = match req {
            ActionRequest::ClickPoint {
                bundle_id,
                x,
                y,
                prefer_ax,
                click_count,
                button,
            } => JsonValue::object(vec![
                ("kind", JsonValue::String("click_point".into())),
                ("bundle_id", JsonValue::String(bundle_id.clone())),
                ("x", JsonValue::Number(*x)),
                ("y", JsonValue::Number(*y)),
                ("prefer_ax", JsonValue::Bool(*prefer_ax)),
                ("click_count", JsonValue::Number(*click_count as f64)),
                ("button", JsonValue::String(button.clone())),
            ]),
            ActionRequest::HoverPoint { bundle_id, x, y } => JsonValue::object(vec![
                ("kind", JsonValue::String("hover_point".into())),
                ("bundle_id", JsonValue::String(bundle_id.clone())),
                ("x", JsonValue::Number(*x)),
                ("y", JsonValue::Number(*y)),
            ]),
            ActionRequest::DragPoints {
                bundle_id,
                x1,
                y1,
                x2,
                y2,
            } => JsonValue::object(vec![
                ("kind", JsonValue::String("drag_points".into())),
                ("bundle_id", JsonValue::String(bundle_id.clone())),
                ("x1", JsonValue::Number(*x1)),
                ("y1", JsonValue::Number(*y1)),
                ("x2", JsonValue::Number(*x2)),
                ("y2", JsonValue::Number(*y2)),
            ]),
            ActionRequest::TypePoint {
                bundle_id,
                x,
                y,
                text,
                mode,
                click_first,
            } => JsonValue::object(vec![
                ("kind", JsonValue::String("type_point".into())),
                ("bundle_id", JsonValue::String(bundle_id.clone())),
                ("x", JsonValue::Number(*x)),
                ("y", JsonValue::Number(*y)),
                ("text", JsonValue::String(text.clone())),
                ("mode", JsonValue::String(mode.clone())),
                ("click_first", JsonValue::Bool(*click_first)),
            ]),
            ActionRequest::PressKey {
                bundle_id,
                key,
                modifiers,
            } => JsonValue::object(vec![
                ("kind", JsonValue::String("press_key".into())),
                ("bundle_id", JsonValue::String(bundle_id.clone())),
                ("key", JsonValue::String(key.clone())),
                (
                    "modifiers",
                    JsonValue::Array(modifiers.iter().cloned().map(JsonValue::String).collect()),
                ),
            ]),
            ActionRequest::Scroll {
                bundle_id,
                direction,
                amount,
                x,
                y,
            } => {
                let mut fields = vec![
                    ("kind", JsonValue::String("scroll".into())),
                    ("bundle_id", JsonValue::String(bundle_id.clone())),
                    ("direction", JsonValue::String(direction.clone())),
                    ("amount", JsonValue::Number(*amount as f64)),
                ];
                if let (Some(px), Some(py)) = (x, y) {
                    fields.push(("x", JsonValue::Number(*px)));
                    fields.push(("y", JsonValue::Number(*py)));
                }
                JsonValue::object(fields)
            }
        };
        let creq = to_cstr(&payload.to_json_string())?;
        let val = take_json(unsafe { apexuse_macos_perform_action(creq.as_ptr()) })?;
        let method = val
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("native")
            .to_string();
        Ok(ActionResult { method })
    }

    fn supports_fast_point_verify(&self) -> bool {
        true
    }

    fn verify_point_safe(&self, bundle_id: &str, x: f64, y: f64) -> Result<bool, DriverError> {
        let c = to_cstr(bundle_id)?;
        let val = take_json(unsafe { apexuse_macos_check_point(c.as_ptr(), x, y) })?;
        Ok(val.get("ok").and_then(|v| v.as_bool()) == Some(true))
    }

    fn open_url(&self, bundle_id: &str, url: &str) -> Result<(), DriverError> {
        let cb = to_cstr(bundle_id)?;
        let cu = to_cstr(url)?;
        take_json(unsafe { apexuse_macos_open_url(cb.as_ptr(), cu.as_ptr()) })?;
        Ok(())
    }

    fn tick(&self) {
        unsafe {
            apexuse_macos_tick();
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::gateway::encode_solid_png_b64;

    #[test]
    fn native_render_som_draws_badges_and_returns_valid_png() {
        let base_b64 = encode_solid_png_b64(240, 140);
        let marks_json = r#"[{"id":"m1","source":"ax","x":20,"y":30,"w":80,"h":30},{"id":"m2","source":"ocr","x":130,"y":60,"w":90,"h":28}]"#;
        let win = Rect {
            x: 0.0,
            y: 0.0,
            w: 240.0,
            h: 140.0,
        };
        let out_b64 = MacosDriver.render_som_png(&base_b64, marks_json, &win).unwrap();
        assert!(!out_b64.is_empty());
        assert_ne!(out_b64, base_b64);
        // Base64 PNG header "iVBORw0KGgo"
        assert!(out_b64.starts_with("iVBORw0KGgo"));
    }
}
