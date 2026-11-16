#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use super::common::{build_snapshot_from_raw, RawElement};
use super::{
    ActionRequest, ActionResult, AppSnapshot, CaptureResult, DoctorStatus, DriverError, OcrMode,
    OsDriver, Rect, RunningApp, WindowMeta,
};
use crate::json::{parse_json, JsonValue};
use crate::som_png::render_som_rgba_to_b64;

fn mask_shift_and_bits(mask: u64) -> (u32, u8) {
    if mask == 0 {
        return (0, 8);
    }
    let shift = mask.trailing_zeros();
    let ones = (mask >> shift).trailing_ones() as u8;
    (shift, ones.max(1))
}

fn extract_channel(pixel: u32, mask: u64) -> u8 {
    if mask == 0 {
        return 0;
    }
    let (shift, bits) = mask_shift_and_bits(mask);
    let raw = ((pixel as u64) & mask) >> shift;
    if bits >= 8 {
        (raw >> (bits - 8)) as u8
    } else {
        ((raw * 255) / ((1u64 << bits) - 1)) as u8
    }
}

/// Converts raw `XGetImage` ZPixmap bytes (24/32 bpp) into packed RGBA pixels.
pub fn extract_ximage_rgba(
    data: &[u8],
    width: usize,
    height: usize,
    bytes_per_line: usize,
    bits_per_pixel: usize,
    red_mask: u64,
    green_mask: u64,
    blue_mask: u64,
) -> Vec<u8> {
    let mut rgba = vec![0u8; width * height * 4];
    let bpp_bytes = (bits_per_pixel / 8).max(1);
    let r_mask = if red_mask != 0 { red_mask } else { 0x00FF_0000 };
    let g_mask = if green_mask != 0 { green_mask } else { 0x0000_FF00 };
    let b_mask = if blue_mask != 0 { blue_mask } else { 0x0000_00FF };

    for y in 0..height {
        let row_off = y * bytes_per_line;
        for x in 0..width {
            let off = row_off + x * bpp_bytes;
            if off + bpp_bytes > data.len() {
                break;
            }
            let pix = match bpp_bytes {
                4 => u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]),
                3 => u32::from_le_bytes([data[off], data[off + 1], data[off + 2], 0]),
                _ => data[off] as u32,
            };
            let di = (y * width + x) * 4;
            rgba[di] = extract_channel(pix, r_mask);
            rgba[di + 1] = extract_channel(pix, g_mask);
            rgba[di + 2] = extract_channel(pix, b_mask);
            rgba[di + 3] = 255;
        }
    }
    rgba
}

/// Parses JSON array payload returned by the Linux AT-SPI2 introspection helper.
pub fn parse_atspi_snapshot_json(
    raw_json: &str,
    bundle_id: &str,
    window_filter: Option<i32>,
    max_elements: i32,
    max_depth: i32,
) -> Result<AppSnapshot, DriverError> {
    let val = parse_json(raw_json)
        .map_err(|e| DriverError::new("atspi_parse_error", format!("Invalid AT-SPI JSON: {e}")))?;
    if val.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let code = val
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("app_not_running");
        let msg = val
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("AT-SPI query failed");
        return Err(DriverError::new(code, msg));
    }

    let resolved_app = val
        .get("app")
        .and_then(|v| v.as_str())
        .unwrap_or(bundle_id)
        .to_string();
    let front_app = val
        .get("frontmost_app")
        .and_then(|v| v.as_str())
        .unwrap_or(&resolved_app)
        .to_string();
    let pid = val.get("pid").and_then(|v| v.as_i64()).unwrap_or(0);
    let frontmost = val.get("frontmost").and_then(|v| v.as_bool()).unwrap_or(true);

    let mut windows = Vec::new();
    if let Some(arr) = val.get("windows").and_then(|v| v.as_array()) {
        for (i, w) in arr.iter().enumerate() {
            let idx = w.get("index").and_then(|v| v.as_i64()).unwrap_or(i as i64) as usize;
            let title = w.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let frame = parse_json_rect(w);
            windows.push(WindowMeta { index: idx, title, frame });
        }
    }
    if let Some(idx) = window_filter {
        if (idx as usize) >= windows.len() && !windows.is_empty() {
            return Err(DriverError::new("window_out_of_range", "Window index out of range"));
        }
    }

    let mut raw_els = Vec::new();
    if let Some(arr) = val.get("elements").and_then(|v| v.as_array()) {
        for el in arr {
            let win_idx = el.get("window").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
            if let Some(wf) = window_filter {
                if win_idx != wf as usize {
                    continue;
                }
            }
            let actions = el
                .get("actions")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            raw_els.push(RawElement {
                role: el.get("role").and_then(|v| v.as_str()).unwrap_or("Element").to_string(),
                label: el.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                value: el.get("value").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string),
                window: win_idx,
                depth: el.get("depth").and_then(|v| v.as_i64()).unwrap_or(1) as usize,
                enabled: el.get("enabled").and_then(|v| v.as_bool()),
                focused: el.get("focused").and_then(|v| v.as_bool()).unwrap_or(false),
                is_password: el.get("is_password").and_then(|v| v.as_bool()).unwrap_or(false),
                offscreen: el.get("offscreen").and_then(|v| v.as_bool()).unwrap_or(false),
                actions,
                frame: parse_json_rect(el),
            });
        }
    }

    Ok(build_snapshot_from_raw(
        resolved_app,
        front_app,
        pid,
        frontmost,
        windows,
        raw_els,
        max_elements,
        max_depth,
    ))
}

fn parse_json_rect(v: &JsonValue) -> Option<Rect> {
    let x = v.get("x").and_then(|n| n.as_f64())?;
    let y = v.get("y").and_then(|n| n.as_f64())?;
    let w = v.get("w").and_then(|n| n.as_f64())?;
    let h = v.get("h").and_then(|n| n.as_f64())?;
    if w > 0.0 && h > 0.0 {
        Some(Rect { x, y, w, h })
    } else {
        None
    }
}

pub fn map_x11_keysym_name(key: &str) -> Option<&'static str> {
    let lower = key.trim().to_ascii_lowercase();
    match lower.as_str() {
        "return" | "enter" => Some("Return"),
        "tab" => Some("Tab"),
        "space" => Some("space"),
        "escape" | "esc" => Some("Escape"),
        "backspace" => Some("BackSpace"),
        "delete" | "forwarddelete" => Some("Delete"),
        "up" => Some("Up"),
        "down" => Some("Down"),
        "left" => Some("Left"),
        "right" => Some("Right"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "pageup" => Some("Prior"),
        "pagedown" => Some("Next"),
        "f1" => Some("F1"),
        "f2" => Some("F2"),
        "f3" => Some("F3"),
        "f4" => Some("F4"),
        "f5" => Some("F5"),
        "f6" => Some("F6"),
        "f7" => Some("F7"),
        "f8" => Some("F8"),
        "f9" => Some("F9"),
        "f10" => Some("F10"),
        "f11" => Some("F11"),
        "f12" => Some("F12"),
        "-" => Some("minus"),
        "=" => Some("equal"),
        "[" => Some("bracketleft"),
        "]" => Some("bracketright"),
        ";" => Some("semicolon"),
        "'" => Some("apostrophe"),
        "\\" => Some("backslash"),
        "," => Some("comma"),
        "." => Some("period"),
        "/" => Some("slash"),
        "`" => Some("grave"),
        _ => None,
    }
}

pub struct LinuxDriver;

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use crate::driver::common::{alias_candidates, app_matches, parse_tesseract_tsv};
    use crate::driver::OcrBox;
    use crate::som_png::{
        cache_rgba_frame, compute_pixel_digest, decode_png_rgba, encode_rgba_png_b64,
        encode_rgba_png_bytes,
    };
    use std::ffi::CString;
    use std::io::Write;
    use std::os::raw::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
    use std::process::{Command, Stdio};
    use std::ptr::{null, null_mut};
    use std::sync::OnceLock;
    use std::time::Duration;

    const RTLD_LAZY: c_int = 1;
    const ZPIXMAP: c_int = 2;
    const ALL_PLANES: c_ulong = !0;
    const CURRENT_TIME: c_ulong = 0;

    #[repr(C)]
    struct XWindowAttributes {
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
        border_width: c_int,
        depth: c_int,
        visual: *mut c_void,
        root: c_ulong,
        class: c_int,
        bit_gravity: c_int,
        win_gravity: c_int,
        backing_store: c_int,
        backing_planes: c_ulong,
        backing_pixel: c_ulong,
        save_under: c_int,
        colormap: c_ulong,
        map_installed: c_int,
        map_state: c_int,
        all_event_masks: c_long,
        your_event_mask: c_long,
        do_not_propagate_mask: c_long,
        override_redirect: c_int,
        screen: *mut c_void,
    }

    #[repr(C)]
    struct XImageFuncs {
        create_image: *const c_void,
        destroy_image: Option<unsafe extern "C" fn(*mut XImage) -> c_int>,
        get_pixel: *const c_void,
        put_pixel: *const c_void,
        sub_image: *const c_void,
        add_pixel: *const c_void,
    }

    #[repr(C)]
    struct XImage {
        width: c_int,
        height: c_int,
        xoffset: c_int,
        format: c_int,
        data: *mut c_char,
        byte_order: c_int,
        bitmap_unit: c_int,
        bitmap_bit_order: c_int,
        bitmap_pad: c_int,
        depth: c_int,
        bytes_per_line: c_int,
        bits_per_pixel: c_int,
        red_mask: c_ulong,
        green_mask: c_ulong,
        blue_mask: c_ulong,
        obdata: *mut c_void,
        f: XImageFuncs,
    }

    #[link(name = "dl")]
    extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    struct X11Api {
        x_open_display: unsafe extern "C" fn(*const c_char) -> *mut c_void,
        x_close_display: unsafe extern "C" fn(*mut c_void) -> c_int,
        x_default_root_window: unsafe extern "C" fn(*mut c_void) -> c_ulong,
        x_default_screen: unsafe extern "C" fn(*mut c_void) -> c_int,
        x_display_width: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
        x_display_height: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
        x_flush: unsafe extern "C" fn(*mut c_void) -> c_int,
        x_sync: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
        x_set_error_handler: unsafe extern "C" fn(Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int>) -> *mut c_void,
        x_intern_atom: unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong,
        x_get_window_property: unsafe extern "C" fn(
            *mut c_void, c_ulong, c_ulong, c_long, c_long, c_int, c_ulong,
            *mut c_ulong, *mut c_int, *mut c_ulong, *mut c_ulong, *mut *mut u8,
        ) -> c_int,
        x_free: unsafe extern "C" fn(*mut c_void) -> c_int,
        x_get_window_attributes: unsafe extern "C" fn(*mut c_void, c_ulong, *mut XWindowAttributes) -> c_int,
        x_translate_coordinates: unsafe extern "C" fn(*mut c_void, c_ulong, c_ulong, c_int, c_int, *mut c_int, *mut c_int, *mut c_ulong) -> c_int,
        x_get_image: unsafe extern "C" fn(*mut c_void, c_ulong, c_int, c_int, c_uint, c_uint, c_ulong, c_int) -> *mut XImage,
        x_raise_window: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int,
        x_set_input_focus: unsafe extern "C" fn(*mut c_void, c_ulong, c_int, c_ulong) -> c_int,
        x_string_to_keysym: unsafe extern "C" fn(*const c_char) -> c_ulong,
        x_keysym_to_keycode: unsafe extern "C" fn(*mut c_void, c_ulong) -> u8,
        xtest_motion: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_ulong) -> c_int>,
        xtest_button: Option<unsafe extern "C" fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int>,
        xtest_key: Option<unsafe extern "C" fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int>,
    }

    unsafe extern "C" fn ignore_x11_error(_d: *mut c_void, _e: *mut c_void) -> c_int {
        0
    }

    unsafe fn sym<T>(lib: *mut c_void, name: &[u8]) -> Option<T> {
        let p = dlsym(lib, name.as_ptr() as *const c_char);
        if p.is_null() {
            None
        } else {
            Some(std::mem::transmute_copy(&p))
        }
    }

    fn x11_api() -> Option<&'static X11Api> {
        static API: OnceLock<Option<X11Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let lib_x11 = dlopen(b"libX11.so.6\0".as_ptr() as *const c_char, RTLD_LAZY);
            if lib_x11.is_null() {
                return None;
            }
            let lib_xtst = dlopen(b"libXtst.so.6\0".as_ptr() as *const c_char, RTLD_LAZY);
            let x_set_error_handler = sym(lib_x11, b"XSetErrorHandler\0")?;
            let api = X11Api {
                x_open_display: sym(lib_x11, b"XOpenDisplay\0")?,
                x_close_display: sym(lib_x11, b"XCloseDisplay\0")?,
                x_default_root_window: sym(lib_x11, b"XDefaultRootWindow\0")?,
                x_default_screen: sym(lib_x11, b"XDefaultScreen\0")?,
                x_display_width: sym(lib_x11, b"XDisplayWidth\0")?,
                x_display_height: sym(lib_x11, b"XDisplayHeight\0")?,
                x_flush: sym(lib_x11, b"XFlush\0")?,
                x_sync: sym(lib_x11, b"XSync\0")?,
                x_set_error_handler,
                x_intern_atom: sym(lib_x11, b"XInternAtom\0")?,
                x_get_window_property: sym(lib_x11, b"XGetWindowProperty\0")?,
                x_free: sym(lib_x11, b"XFree\0")?,
                x_get_window_attributes: sym(lib_x11, b"XGetWindowAttributes\0")?,
                x_translate_coordinates: sym(lib_x11, b"XTranslateCoordinates\0")?,
                x_get_image: sym(lib_x11, b"XGetImage\0")?,
                x_raise_window: sym(lib_x11, b"XRaiseWindow\0")?,
                x_set_input_focus: sym(lib_x11, b"XSetInputFocus\0")?,
                x_string_to_keysym: sym(lib_x11, b"XStringToKeysym\0")?,
                x_keysym_to_keycode: sym(lib_x11, b"XKeysymToKeycode\0")?,
                xtest_motion: if lib_xtst.is_null() { None } else { sym(lib_xtst, b"XTestFakeMotionEvent\0") },
                xtest_button: if lib_xtst.is_null() { None } else { sym(lib_xtst, b"XTestFakeButtonEvent\0") },
                xtest_key: if lib_xtst.is_null() { None } else { sym(lib_xtst, b"XTestFakeKeyEvent\0") },
            };
            (api.x_set_error_handler)(Some(ignore_x11_error));
            Some(api)
        })
        .as_ref()
    }

    struct DisplayGuard {
        api: &'static X11Api,
        dpy: *mut c_void,
    }
    impl DisplayGuard {
        fn open() -> Option<Self> {
            let api = x11_api()?;
            let dpy = unsafe { (api.x_open_display)(null()) };
            if dpy.is_null() {
                None
            } else {
                Some(Self { api, dpy })
            }
        }
    }
    impl Drop for DisplayGuard {
        fn drop(&mut self) {
            unsafe {
                (self.api.x_close_display)(self.dpy);
            }
        }
    }

    #[derive(Clone)]
    struct XWin {
        xid: c_ulong,
        pid: i64,
        app_id: String,
        wm_class: String,
        title: String,
        rect: Rect,
        active: bool,
    }

    fn cmd_exists(bin: &str) -> bool {
        Command::new("sh")
            .args(["-c", &format!("command -v {bin} >/dev/null 2>&1")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn is_wayland_session() -> bool {
        let st = std::env::var("XDG_SESSION_TYPE").unwrap_or_default().to_ascii_lowercase();
        let wl = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
        st == "wayland" || !wl.is_empty()
    }

    fn read_proc_comm(pid: i64) -> String {
        if pid <= 0 {
            return String::new();
        }
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    unsafe fn x11_get_prop_bytes(
        api: &X11Api,
        dpy: *mut c_void,
        win: c_ulong,
        atom_name: &[u8],
    ) -> Option<(c_int, Vec<u8>)> {
        let atom = (api.x_intern_atom)(dpy, atom_name.as_ptr() as *const c_char, 1);
        if atom == 0 {
            return None;
        }
        let mut actual_type: c_ulong = 0;
        let mut actual_format: c_int = 0;
        let mut nitems: c_ulong = 0;
        let mut bytes_after: c_ulong = 0;
        let mut prop: *mut u8 = null_mut();
        let rc = (api.x_get_window_property)(
            dpy,
            win,
            atom,
            0,
            65536,
            0,
            0,
            &mut actual_type,
            &mut actual_format,
            &mut nitems,
            &mut bytes_after,
            &mut prop,
        );
        if rc != 0 || prop.is_null() {
            return None;
        }
        let elem_bytes = match actual_format {
            32 => std::mem::size_of::<c_long>(),
            16 => 2,
            _ => 1,
        };
        let slice = std::slice::from_raw_parts(prop, (nitems as usize) * elem_bytes);
        let vec = slice.to_vec();
        (api.x_free)(prop as *mut c_void);
        Some((actual_format, vec))
    }

    fn list_x11_windows() -> Vec<XWin> {
        let Some(g) = DisplayGuard::open() else {
            return Vec::new();
        };
        unsafe {
            let root = (g.api.x_default_root_window)(g.dpy);
            let active_xid = x11_get_prop_bytes(g.api, g.dpy, root, b"_NET_ACTIVE_WINDOW\0")
                .and_then(|(_, b)| {
                    if b.len() >= std::mem::size_of::<c_long>() {
                        Some(c_long::from_ne_bytes(b[..std::mem::size_of::<c_long>()].try_into().ok()?) as c_ulong)
                    } else {
                        None
                    }
                })
                .unwrap_or(0);

            let Some((_, list_bytes)) = x11_get_prop_bytes(g.api, g.dpy, root, b"_NET_CLIENT_LIST\0") else {
                return Vec::new();
            };
            let stride = std::mem::size_of::<c_long>();
            let mut wins = Vec::new();
            for chunk in list_bytes.chunks_exact(stride) {
                let Ok(arr) = chunk.try_into() else { continue };
                let xid = c_long::from_ne_bytes(arr) as c_ulong;
                let mut attrs: XWindowAttributes = std::mem::zeroed();
                if (g.api.x_get_window_attributes)(g.dpy, xid, &mut attrs) == 0 || attrs.map_state != 2 {
                    continue;
                }
                let mut rx = 0i32;
                let mut ry = 0i32;
                let mut child = 0;
                (g.api.x_translate_coordinates)(g.dpy, xid, root, 0, 0, &mut rx, &mut ry, &mut child);
                let pid = x11_get_prop_bytes(g.api, g.dpy, xid, b"_NET_WM_PID\0")
                    .and_then(|(_, b)| {
                        b.get(..stride)
                            .and_then(|s| s.try_into().ok())
                            .map(|a| c_long::from_ne_bytes(a) as i64)
                    })
                    .unwrap_or(0);
                let wm_class = x11_get_prop_bytes(g.api, g.dpy, xid, b"WM_CLASS\0")
                    .map(|(_, b)| {
                        let parts: Vec<&str> = b
                            .split(|&c| c == 0)
                            .filter(|s| !s.is_empty())
                            .filter_map(|s| std::str::from_utf8(s).ok())
                            .collect();
                        parts.last().copied().unwrap_or("").to_string()
                    })
                    .unwrap_or_default();
                let title = x11_get_prop_bytes(g.api, g.dpy, xid, b"_NET_WM_NAME\0")
                    .or_else(|| x11_get_prop_bytes(g.api, g.dpy, xid, b"WM_NAME\0"))
                    .map(|(_, b)| String::from_utf8_lossy(&b).trim_end_matches('\0').to_string())
                    .unwrap_or_default();

                let comm = read_proc_comm(pid);
                let app_id = if !comm.is_empty() {
                    comm
                } else if !wm_class.is_empty() {
                    wm_class.clone()
                } else {
                    format!("pid:{pid}")
                };

                if attrs.width > 1 && attrs.height > 1 {
                    wins.push(XWin {
                        xid,
                        pid,
                        app_id,
                        wm_class,
                        title,
                        rect: Rect {
                            x: rx as f64,
                            y: ry as f64,
                            w: attrs.width as f64,
                            h: attrs.height as f64,
                        },
                        active: xid == active_xid,
                    });
                }
            }
            wins
        }
    }

    const ATSPI_HELPER_PY: &str = r#"
import json, os, sys

def read_comm(pid):
    try:
        with open(f"/proc/{pid}/comm", "r") as f:
            return f.read().strip()
    except Exception:
        return ""

def run():
    try:
        import gi
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi
    except Exception as e:
        print(json.dumps({"ok": False, "code": "accessibility_unavailable", "message": str(e)}))
        return

    cmd = sys.argv[1] if len(sys.argv) > 1 else "list"
    target = sys.argv[2] if len(sys.argv) > 2 else ""
    max_el = int(sys.argv[3]) if len(sys.argv) > 3 else 150
    max_depth = int(sys.argv[4]) if len(sys.argv) > 4 else 12

    desktop = Atspi.get_desktop(0)
    n_apps = desktop.get_child_count()

    if cmd == "doctor":
        print(json.dumps({"ok": True, "apps_count": n_apps}))
        return

    def app_entry(app):
        name = (app.get_name() or "").strip()
        pid = 0
        try:
            pid = app.get_process_id()
        except Exception:
            pass
        comm = read_comm(pid)
        active = False
        windows = []
        wc = app.get_child_count()
        for i in range(wc):
            w = app.get_child_at_index(i)
            if not w:
                continue
            try:
                ss = w.get_state_set()
                is_active = ss.contains(Atspi.StateType.ACTIVE)
                showing = ss.contains(Atspi.StateType.SHOWING)
                if not showing and not is_active:
                    continue
                if is_active:
                    active = True
                rect = w.get_extents(Atspi.CoordType.SCREEN)
                if rect.width <= 1 or rect.height <= 1:
                    continue
                windows.append((len(windows), w, w.get_name() or "", rect, is_active))
            except Exception:
                continue
        return {
            "name": name,
            "comm": comm,
            "pid": pid,
            "active": active,
            "windows": windows,
        }

    entries = []
    front_name = ""
    for i in range(n_apps):
        a = desktop.get_child_at_index(i)
        if not a:
            continue
        ent = app_entry(a)
        if ent["windows"]:
            if ent["active"]:
                front_name = ent["comm"] or ent["name"]
            entries.append(ent)

    if cmd == "list":
        apps = []
        seen = set()
        for ent in entries:
            aid = ent["comm"] or ent["name"] or f"pid:{ent['pid']}"
            k = aid.lower()
            if k in seen:
                continue
            seen.add(k)
            apps.append({"bundle_id": aid, "pid": ent["pid"], "active": ent["active"]})
        print(json.dumps({"ok": True, "apps": apps}))
        return

    cands = [c.lower() for c in json.loads(os.environ.get("APEXUSE_CANDS", "[]"))]
    chosen = None
    if target.lower() == "desktop":
        chosen = next((e for e in entries if e["active"]), entries[0] if entries else None)
    elif target.startswith("pid:"):
        tpid = int(target[4:])
        chosen = next((e for e in entries if e["pid"] == tpid), None)
    else:
        for ent in entries:
            nm = ent["name"].lower()
            cm = ent["comm"].lower()
            if nm in cands or cm in cands or any(w[2].lower().find(c) >= 0 and len(c) >= 4 for w in ent["windows"] for c in cands):
                chosen = ent
                break

    if not chosen:
        print(json.dumps({"ok": False, "code": "app_not_running", "message": f"App '{target}' not found in AT-SPI"}))
        return

    win_metas = []
    elements = []
    for idx, w, title, rect, is_active in chosen["windows"]:
        win_metas.append({"index": idx, "title": title, "x": rect.x, "y": rect.y, "w": rect.width, "h": rect.height})

    def walk(node, win_idx, depth):
        if len(elements) >= max_el + 40 or depth > max_depth + 1 or not node:
            return
        try:
            role_obj = node.get_role()
            role = (node.get_role_name() or "").strip()
            label = (node.get_name() or "").strip()
            ss = node.get_state_set()
            enabled = ss.contains(Atspi.StateType.ENABLED) and ss.contains(Atspi.StateType.SENSITIVE)
            focused = ss.contains(Atspi.StateType.FOCUSED)
            showing = ss.contains(Atspi.StateType.SHOWING)
            is_pwd = (role_obj == Atspi.Role.PASSWORD_TEXT)
            val = None
            if not is_pwd:
                try:
                    ti = node.get_text_iface()
                    if ti:
                        cnt = min(ti.get_character_count(), 256)
                        if cnt > 0:
                            val = ti.get_text(0, cnt)
                except Exception:
                    pass
            acts = []
            try:
                ai = node.get_action_iface()
                if ai:
                    for k in range(ai.get_n_actions()):
                        an = ai.get_action_name(k)
                        if an:
                            acts.append(an)
            except Exception:
                pass
            rect = None
            try:
                ci = node.get_component_iface()
                if ci:
                    r = ci.get_extents(Atspi.CoordType.SCREEN)
                    if r.width > 0 and r.height > 0:
                        rect = (r.x, r.y, r.width, r.height)
            except Exception:
                pass
            item = {
                "role": role,
                "label": label,
                "value": val,
                "window": win_idx,
                "depth": depth,
                "enabled": bool(enabled),
                "focused": bool(focused),
                "is_password": bool(is_pwd),
                "offscreen": not bool(showing),
                "actions": acts,
            }
            if rect:
                item["x"], item["y"], item["w"], item["h"] = rect
            elements.append(item)
            cc = min(node.get_child_count(), 120)
            for ci in range(cc):
                walk(node.get_child_at_index(ci), win_idx, depth + 1)
        except Exception:
            pass

    if cmd == "inspect":
        for idx, w, title, rect, _ in chosen["windows"]:
            walk(w, idx, 1)
    elif cmd == "do_action":
        x, y = float(sys.argv[3]), float(sys.argv[4])
        done = False
        def find_click(node):
            nonlocal done
            if done or not node:
                return
            try:
                ci = node.get_component_iface()
                if ci:
                    r = ci.get_extents(Atspi.CoordType.SCREEN)
                    if r.x <= x <= r.x + r.width and r.y <= y <= r.y + r.height:
                        if node.get_role() == Atspi.Role.PASSWORD_TEXT:
                            done = "password"
                            return
                        cc = node.get_child_count()
                        for i in range(cc):
                            find_click(node.get_child_at_index(i))
                            if done:
                                return
                        ai = node.get_action_iface()
                        if ai and ai.get_n_actions() > 0:
                            ai.do_action(0)
                            done = "ok"
                            return
            except Exception:
                pass
        for _, w, _, _, _ in chosen["windows"]:
            find_click(w)
            if done:
                break
        print(json.dumps({"ok": done == "ok", "status": done or "miss"}))
        return

    aid = chosen["comm"] or chosen["name"] or target
    print(json.dumps({
        "ok": True,
        "app": aid,
        "frontmost_app": front_app or aid,
        "pid": chosen["pid"],
        "frontmost": chosen["active"],
        "windows": win_metas,
        "elements": elements,
    }))

run()
"#;

    fn run_atspi_py(cmd: &str, args: &[&str], cands_json: Option<&str>) -> Option<String> {
        let mut command = Command::new("python3");
        command.arg("-c").arg(ATSPI_HELPER_PY).arg(cmd);
        for a in args {
            command.arg(a);
        }
        if let Some(cj) = cands_json {
            command.env("APEXUSE_CANDS", cj);
        }
        let out = command.output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    pub fn doctor() -> DoctorStatus {
        let atspi_ok = run_atspi_py("doctor", &[], None)
            .and_then(|s| parse_json(&s).ok())
            .and_then(|v| v.get("ok").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        let x11_ok = DisplayGuard::open().is_some();
        let wl = is_wayland_session();
        let wl_cap = cmd_exists("grim") || cmd_exists("spectacle") || cmd_exists("gnome-screenshot");
        let scr_ok = x11_ok || (wl && wl_cap);
        let tess_ok = cmd_exists("tesseract");

        let note = format!(
            "Linux session={} x11={} atspi={} wayland_cap={} tesseract={}",
            if wl { "wayland" } else { "x11" },
            x11_ok,
            atspi_ok,
            wl_cap,
            tess_ok
        );
        DoctorStatus {
            os: "linux".into(),
            accessibility: atspi_ok,
            screen_recording: scr_ok,
            note,
        }
    }

    pub fn list_apps() -> Result<Vec<RunningApp>, DriverError> {
        let mut seen = std::collections::HashSet::new();
        let mut apps = Vec::new();
        if let Some(raw) = run_atspi_py("list", &[], None) {
            if let Ok(val) = parse_json(&raw) {
                if val.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    if let Some(arr) = val.get("apps").and_then(|v| v.as_array()) {
                        for item in arr {
                            let bid = item.get("bundle_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let pid = item.get("pid").and_then(|v| v.as_i64()).unwrap_or(0);
                            let active = item.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
                            if !bid.is_empty() && seen.insert(bid.to_ascii_lowercase()) {
                                apps.push(RunningApp { bundle_id: bid, pid, active });
                            }
                        }
                    }
                }
            }
        }
        for w in list_x11_windows() {
            if seen.insert(w.app_id.to_ascii_lowercase()) {
                apps.push(RunningApp {
                    bundle_id: w.app_id,
                    pid: w.pid,
                    active: w.active,
                });
            }
        }
        Ok(apps)
    }

    pub fn activate_app(bundle_id: &str) -> Result<(), DriverError> {
        let xwins = list_x11_windows();
        if let Some(w) = xwins.iter().find(|w| {
            app_matches(bundle_id, &w.app_id, Some(&w.wm_class), Some(&w.title), w.pid)
        }) {
            if cmd_exists("wmctrl") {
                let _ = Command::new("wmctrl")
                    .args(["-i", "-a", &format!("0x{:x}", w.xid)])
                    .status();
                return Ok(());
            }
            if let Some(g) = DisplayGuard::open() {
                unsafe {
                    (g.api.x_raise_window)(g.dpy, w.xid);
                    (g.api.x_set_input_focus)(g.dpy, w.xid, 1, CURRENT_TIME);
                    (g.api.x_flush)(g.dpy);
                }
                return Ok(());
            }
        }
        if cmd_exists("xdotool") {
            let status = Command::new("xdotool")
                .args(["search", "--class", bundle_id, "windowactivate"])
                .status();
            if status.map(|s| s.success()).unwrap_or(false) {
                return Ok(());
            }
        }
        Err(DriverError::new(
            "app_not_running",
            format!("Could not focus Linux application '{bundle_id}'"),
        ))
    }

    pub fn launch_app(bundle_id: &str) -> Result<(), DriverError> {
        for cand in alias_candidates(bundle_id) {
            if cmd_exists(&cand) {
                Command::new(&cand)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| DriverError::new("launch_failed", e.to_string()))?;
                return Ok(());
            }
        }
        if cmd_exists("gtk-launch") {
            let status = Command::new("gtk-launch").arg(bundle_id).status();
            if status.map(|s| s.success()).unwrap_or(false) {
                return Ok(());
            }
        }
        Err(DriverError::new(
            "launch_failed",
            format!("Executable or .desktop launcher not found for '{bundle_id}'"),
        ))
    }

    pub fn open_url(bundle_id: &str, url: &str) -> Result<(), DriverError> {
        if !bundle_id.is_empty() && !bundle_id.eq_ignore_ascii_case("desktop") {
            for cand in alias_candidates(bundle_id) {
                if matches!(
                    cand.as_str(),
                    "google-chrome" | "google-chrome-stable" | "chromium" | "chromium-browser" | "firefox" | "firefox-esr" | "microsoft-edge"
                ) && cmd_exists(&cand)
                {
                    Command::new(&cand)
                        .arg(url)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .map_err(|e| DriverError::new("launch_failed", e.to_string()))?;
                    return Ok(());
                }
            }
        }
        Command::new("xdg-open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| DriverError::new("launch_failed", e.to_string()))?;
        Ok(())
    }

    pub fn inspect_app(
        bundle_id: &str,
        window: Option<i32>,
        max_elements: i32,
        max_depth: i32,
    ) -> Result<AppSnapshot, DriverError> {
        let cands = JsonValue::Array(
            alias_candidates(bundle_id)
                .into_iter()
                .map(JsonValue::String)
                .collect(),
        )
        .to_json_string();
        let mel = max_elements.to_string();
        let md = max_depth.to_string();
        if let Some(raw) = run_atspi_py("inspect", &[bundle_id, &mel, &md], Some(&cands)) {
            if let Ok(snap) = parse_atspi_snapshot_json(&raw, bundle_id, window, max_elements, max_depth) {
                if !snap.elements.is_empty() || !snap.windows.is_empty() {
                    return Ok(snap);
                }
            }
        }

        // Fallback to X11 window metadata when AT-SPI is unavailable or app exposes no AX tree.
        let xwins = list_x11_windows();
        let front = xwins
            .iter()
            .find(|w| w.active)
            .map(|w| w.app_id.clone())
            .unwrap_or_else(|| bundle_id.into());
        let matched: Vec<XWin> = if bundle_id.eq_ignore_ascii_case("desktop") {
            let fg: Vec<XWin> = xwins.iter().filter(|w| w.active).cloned().collect();
            if fg.is_empty() { xwins } else { fg }
        } else {
            xwins
                .into_iter()
                .filter(|w| app_matches(bundle_id, &w.app_id, Some(&w.wm_class), Some(&w.title), w.pid))
                .collect()
        };
        if matched.is_empty() {
            return Err(DriverError::new(
                "app_not_running",
                format!("No visible Linux window matching '{bundle_id}'"),
            ));
        }
        let pid = matched[0].pid;
        let is_fg = matched.iter().any(|w| w.active);
        let metas: Vec<WindowMeta> = matched
            .iter()
            .enumerate()
            .map(|(i, w)| WindowMeta {
                index: i,
                title: w.title.clone(),
                frame: Some(w.rect.clone()),
            })
            .collect();
        if let Some(wi) = window {
            if (wi as usize) >= metas.len() {
                return Err(DriverError::new("window_out_of_range", "Window index out of range"));
            }
        }
        Ok(build_snapshot_from_raw(
            matched[0].app_id.clone(),
            front,
            pid,
            is_fg,
            metas,
            Vec::new(),
            max_elements,
            max_depth,
        ))
    }

    fn capture_x11_rect(rect: Option<Rect>) -> Result<(Rect, usize, usize, Vec<u8>), DriverError> {
        let g = DisplayGuard::open()
            .ok_or_else(|| DriverError::new("capture_failed", "No X11 display available"))?;
        unsafe {
            let root = (g.api.x_default_root_window)(g.dpy);
            let scr = (g.api.x_default_screen)(g.dpy);
            let full_w = (g.api.x_display_width)(g.dpy, scr).max(1) as f64;
            let full_h = (g.api.x_display_height)(g.dpy, scr).max(1) as f64;
            let r = rect.unwrap_or(Rect {
                x: 0.0,
                y: 0.0,
                w: full_w,
                h: full_h,
            });
            let rx = r.x.round().max(0.0) as c_int;
            let ry = r.y.round().max(0.0) as c_int;
            let rw = r.w.round().clamp(1.0, full_w) as c_uint;
            let rh = r.h.round().clamp(1.0, full_h) as c_uint;
            let img = (g.api.x_get_image)(g.dpy, root, rx, ry, rw, rh, ALL_PLANES, ZPIXMAP);
            if img.is_null() {
                return Err(DriverError::new("capture_failed", "XGetImage failed"));
            }
            let xi = &*img;
            let w = xi.width.max(1) as usize;
            let h = xi.height.max(1) as usize;
            let bpl = xi.bytes_per_line.max(1) as usize;
            let bpp = xi.bits_per_pixel.max(8) as usize;
            let total_bytes = bpl * h;
            let raw = std::slice::from_raw_parts(xi.data as *const u8, total_bytes);
            let rgba = extract_ximage_rgba(
                raw,
                w,
                h,
                bpl,
                bpp,
                xi.red_mask as u64,
                xi.green_mask as u64,
                xi.blue_mask as u64,
            );
            if let Some(destroy) = xi.f.destroy_image {
                destroy(img);
            }
            Ok((
                Rect {
                    x: rx as f64,
                    y: ry as f64,
                    w: w as f64,
                    h: h as f64,
                },
                w,
                h,
                rgba,
            ))
        }
    }

    fn crop_rgba_region(
        src: &[u8],
        sw: usize,
        sh: usize,
        rect: Option<Rect>,
    ) -> (Rect, usize, usize, Vec<u8>) {
        let Some(r) = rect else {
            return (
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: sw as f64,
                    h: sh as f64,
                },
                sw,
                sh,
                src.to_vec(),
            );
        };
        let x0 = (r.x.round() as isize).clamp(0, sw as isize - 1) as usize;
        let y0 = (r.y.round() as isize).clamp(0, sh as isize - 1) as usize;
        let cw = (r.w.round() as usize).clamp(1, sw.saturating_sub(x0).max(1));
        let ch = (r.h.round() as usize).clamp(1, sh.saturating_sub(y0).max(1));
        let mut out = Vec::with_capacity(cw * ch * 4);
        for row in y0..(y0 + ch).min(sh) {
            let start = (row * sw + x0) * 4;
            let end = start + cw * 4;
            if end <= src.len() {
                out.extend_from_slice(&src[start..end]);
            }
        }
        (
            Rect {
                x: x0 as f64,
                y: y0 as f64,
                w: cw as f64,
                h: ch as f64,
            },
            cw,
            ch,
            out,
        )
    }

    fn capture_wayland_png(rect: Option<Rect>) -> Result<(Rect, usize, usize, Vec<u8>), DriverError> {
        if cmd_exists("grim") {
            let mut cmd = Command::new("grim");
            if let Some(ref r) = rect {
                cmd.args([
                    "-g",
                    &format!(
                        "{},{} {}x{}",
                        r.x.round() as i32,
                        r.y.round() as i32,
                        r.w.round().max(1.0) as i32,
                        r.h.round().max(1.0) as i32
                    ),
                ]);
            }
            cmd.arg("-");
            let out = cmd
                .output()
                .map_err(|e| DriverError::new("capture_failed", e.to_string()))?;
            if out.status.success() {
                let (w, h, rgba) = decode_png_rgba(&out.stdout)
                    .map_err(|e| DriverError::new("capture_failed", e))?;
                let frame = rect.unwrap_or(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: w as f64,
                    h: h as f64,
                });
                return Ok((frame, w, h, rgba));
            }
        }

        let tmp = std::env::temp_dir().join(format!("apexuse_wl_cap_{}.png", std::process::id()));
        let tmp_str = tmp.to_string_lossy().to_string();
        let captured = if cmd_exists("spectacle") {
            Command::new("spectacle")
                .args(["-b", "-n", "-o", &tmp_str])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        } else if cmd_exists("gnome-screenshot") {
            Command::new("gnome-screenshot")
                .args(["-f", &tmp_str])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        } else {
            false
        };
        if captured {
            let bytes = std::fs::read(&tmp);
            let _ = std::fs::remove_file(&tmp);
            if let Ok(png) = bytes {
                if let Ok((sw, sh, full_rgba)) = decode_png_rgba(&png) {
                    return Ok(crop_rgba_region(&full_rgba, sw, sh, rect));
                }
            }
        } else {
            let _ = std::fs::remove_file(&tmp);
        }

        Err(DriverError::new(
            "capture_failed",
            "Wayland screen capture requires grim, spectacle, or gnome-screenshot (or an active X11 display)",
        ))
    }

    fn run_tesseract(w: usize, h: usize, rgba: &[u8], origin_x: f64, origin_y: f64, mode: OcrMode) -> Vec<OcrBox> {
        if mode == OcrMode::Off || !cmd_exists("tesseract") {
            return Vec::new();
        }
        let png = encode_rgba_png_bytes(w, h, rgba);
        if png.is_empty() {
            return Vec::new();
        }
        let psm = if mode == OcrMode::Accurate { "3" } else { "11" };
        let Ok(mut child) = Command::new("tesseract")
            .args(["stdin", "stdout", "--psm", psm, "tsv"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return Vec::new();
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&png);
        }
        match child.wait_with_output() {
            Ok(out) if out.status.success() => {
                let tsv = String::from_utf8_lossy(&out.stdout);
                parse_tesseract_tsv(&tsv, origin_x, origin_y, 1.0, 1.0)
            }
            _ => Vec::new(),
        }
    }

    pub fn capture_and_ocr(
        bundle_id: &str,
        window: Option<i32>,
        ocr_mode: OcrMode,
        encode_png: bool,
    ) -> Result<CaptureResult, DriverError> {
        let is_desktop = bundle_id.eq_ignore_ascii_case("desktop");
        let (target_rect, out_bundle, front_bundle, pid, wid) = if is_desktop && window.is_none() {
            let xwins = list_x11_windows();
            let fg = xwins.iter().find(|w| w.active).map(|w| w.app_id.clone()).unwrap_or_default();
            (None, "desktop".to_string(), fg, 0, 0u64)
        } else {
            let snap = inspect_app(bundle_id, window, 40, 4)?;
            let chosen = match window {
                Some(idx) => snap.windows.get(idx as usize).and_then(|w| w.frame.clone()),
                None => snap
                    .windows
                    .iter()
                    .filter_map(|w| w.frame.clone())
                    .max_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h))),
            };
            (
                chosen,
                snap.bundle_id,
                snap.frontmost_bundle_id,
                snap.pid,
                0u64,
            )
        };

        let (frame, w, h, rgba) = if DisplayGuard::open().is_some() && !is_wayland_session() {
            capture_x11_rect(target_rect.clone())?
        } else {
            capture_wayland_png(target_rect.clone()).or_else(|_| capture_x11_rect(target_rect))?
        };

        let pixel_digest = compute_pixel_digest(w, h, &rgba);
        let ocr_boxes = run_tesseract(w, h, &rgba, frame.x, frame.y, ocr_mode);
        let png_base64 = if encode_png {
            encode_rgba_png_b64(w, h, &rgba)
        } else {
            String::new()
        };
        let capture_id = cache_rgba_frame("lin", w, h, rgba);

        Ok(CaptureResult {
            bundle_id: out_bundle,
            frontmost_bundle_id: front_bundle,
            pid,
            window_id: wid,
            capture_id: Some(capture_id),
            pixel_digest: Some(pixel_digest),
            window_frame: frame,
            image_width: w,
            image_height: h,
            png_base64,
            ocr_boxes,
        })
    }

    fn send_xtest_key_sym(g: &DisplayGuard, sym_name: &str, press: bool) -> bool {
        let Ok(cstr) = CString::new(sym_name) else {
            return false;
        };
        let Some(xtest_key) = g.api.xtest_key else {
            return false;
        };
        unsafe {
            let ks = (g.api.x_string_to_keysym)(cstr.as_ptr());
            if ks == 0 {
                return false;
            }
            let kc = (g.api.x_keysym_to_keycode)(g.dpy, ks);
            if kc == 0 {
                return false;
            }
            xtest_key(g.dpy, kc as c_uint, if press { 1 } else { 0 }, CURRENT_TIME);
            true
        }
    }

    fn xtest_click(x: f64, y: f64, btn: c_uint, count: u8) -> Result<(), DriverError> {
        let g = DisplayGuard::open().ok_or_else(|| DriverError::new("input_failed", "No X11 display"))?;
        let motion = g.api.xtest_motion.ok_or_else(|| DriverError::new("input_failed", "libXtst unavailable"))?;
        let button = g.api.xtest_button.ok_or_else(|| DriverError::new("input_failed", "libXtst unavailable"))?;
        unsafe {
            motion(g.dpy, -1, x.round() as c_int, y.round() as c_int, CURRENT_TIME);
            for _ in 0..count.max(1) {
                button(g.dpy, btn, 1, CURRENT_TIME);
                button(g.dpy, btn, 0, CURRENT_TIME);
            }
            (g.api.x_sync)(g.dpy, 0);
        }
        Ok(())
    }

    pub fn perform_action(req: &ActionRequest) -> Result<ActionResult, DriverError> {
        match req {
            ActionRequest::ClickPoint {
                bundle_id,
                x,
                y,
                prefer_ax,
                click_count,
                button,
            } => {
                if *prefer_ax && *click_count == 1 && button == "left" {
                    let cands = JsonValue::Array(
                        alias_candidates(bundle_id).into_iter().map(JsonValue::String).collect(),
                    )
                    .to_json_string();
                    let xs = x.round().to_string();
                    let ys = y.round().to_string();
                    if let Some(res) = run_atspi_py("do_action", &[bundle_id, &xs, &ys], Some(&cands)) {
                        if let Ok(v) = parse_json(&res) {
                            if v.get("status").and_then(|s| s.as_str()) == Some("password") {
                                return Err(DriverError::new("unsafe_target_field", "Target element is a password field"));
                            }
                            if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
                                return Ok(ActionResult {
                                    method: "linux_atspi_action".into(),
                                });
                            }
                        }
                    }
                }
                let _ = activate_app(bundle_id);
                let btn = if button == "right" { 3 } else { 1 };
                if xtest_click(*x, *y, btn, *click_count).is_ok() {
                    return Ok(ActionResult {
                        method: "linux_xtest".into(),
                    });
                }
                if cmd_exists("ydotool") {
                    let _ = Command::new("ydotool")
                        .args(["mousemove", "--absolute", "-x", &x.round().to_string(), "-y", &y.round().to_string()])
                        .status();
                    let _ = Command::new("ydotool")
                        .args(["click", if btn == 3 { "0xC1" } else { "0xC0" }])
                        .status();
                    return Ok(ActionResult {
                        method: "linux_ydotool".into(),
                    });
                }
                Err(DriverError::new(
                    "input_failed",
                    "Pointer input failed: XTest and ydotool unavailable",
                ))
            }
            ActionRequest::HoverPoint { x, y, .. } => {
                if let Some(g) = DisplayGuard::open() {
                    if let Some(motion) = g.api.xtest_motion {
                        unsafe {
                            motion(g.dpy, -1, x.round() as c_int, y.round() as c_int, CURRENT_TIME);
                            (g.api.x_sync)(g.dpy, 0);
                        }
                        return Ok(ActionResult { method: "linux_hover".into() });
                    }
                }
                if cmd_exists("ydotool") {
                    let ok = Command::new("ydotool")
                        .args(["mousemove", "--absolute", "-x", &x.round().to_string(), "-y", &y.round().to_string()])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if ok {
                        return Ok(ActionResult { method: "linux_ydotool_hover".into() });
                    }
                }
                Err(DriverError::new("input_failed", "Hover requires XTest or ydotool"))
            }
            ActionRequest::DragPoints {
                bundle_id,
                x1,
                y1,
                x2,
                y2,
            } => {
                let _ = activate_app(bundle_id);
                let g = DisplayGuard::open().ok_or_else(|| DriverError::new("input_failed", "No X11 display"))?;
                let motion = g.api.xtest_motion.ok_or_else(|| DriverError::new("input_failed", "libXtst unavailable"))?;
                let button = g.api.xtest_button.ok_or_else(|| DriverError::new("input_failed", "libXtst unavailable"))?;
                unsafe {
                    motion(g.dpy, -1, x1.round() as c_int, y1.round() as c_int, CURRENT_TIME);
                    button(g.dpy, 1, 1, CURRENT_TIME);
                    (g.api.x_flush)(g.dpy);
                    for s in 1..=8 {
                        let t = s as f64 / 8.0;
                        motion(
                            g.dpy,
                            -1,
                            (x1 + (x2 - x1) * t).round() as c_int,
                            (y1 + (y2 - y1) * t).round() as c_int,
                            CURRENT_TIME,
                        );
                        (g.api.x_flush)(g.dpy);
                        std::thread::sleep(Duration::from_millis(12));
                    }
                    button(g.dpy, 1, 0, CURRENT_TIME);
                    (g.api.x_sync)(g.dpy, 0);
                }
                Ok(ActionResult { method: "linux_drag".into() })
            }
            ActionRequest::TypePoint {
                bundle_id,
                x,
                y,
                text,
                mode,
                click_first,
            } => {
                let _ = activate_app(bundle_id);
                if *click_first {
                    let _ = xtest_click(*x, *y, 1, 1);
                    std::thread::sleep(Duration::from_millis(35));
                }
                if mode == "replace" {
                    let _ = perform_action(&ActionRequest::PressKey {
                        bundle_id: bundle_id.clone(),
                        key: "a".into(),
                        modifiers: vec!["ctrl".into()],
                    });
                    let _ = perform_action(&ActionRequest::PressKey {
                        bundle_id: bundle_id.clone(),
                        key: "backspace".into(),
                        modifiers: vec![],
                    });
                    std::thread::sleep(Duration::from_millis(20));
                }
                if cmd_exists("wtype") && is_wayland_session() {
                    let status = Command::new("wtype").args(["--", text]).status();
                    if status.map(|s| s.success()).unwrap_or(false) {
                        return Ok(ActionResult { method: "linux_wtype".into() });
                    }
                }
                if cmd_exists("xdotool") {
                    let status = Command::new("xdotool")
                        .args(["type", "--clearmodifiers", "--delay", "2", "--", text])
                        .status();
                    if status.map(|s| s.success()).unwrap_or(false) {
                        return Ok(ActionResult { method: "linux_xdotool_type".into() });
                    }
                }
                if let Some(g) = DisplayGuard::open() {
                    for ch in text.chars() {
                        let sym = if ch == '\n' {
                            "Return".to_string()
                        } else if ch == ' ' {
                            "space".to_string()
                        } else if let Some(named) = map_x11_keysym_name(&ch.to_string()) {
                            named.to_string()
                        } else if ch.is_ascii_alphanumeric() {
                            ch.to_string()
                        } else {
                            format!("U{:04x}", ch as u32)
                        };
                        let need_shift = ch.is_ascii_uppercase();
                        if need_shift {
                            send_xtest_key_sym(&g, "Shift_L", true);
                        }
                        send_xtest_key_sym(&g, &sym, true);
                        send_xtest_key_sym(&g, &sym, false);
                        if need_shift {
                            send_xtest_key_sym(&g, "Shift_L", false);
                        }
                    }
                    unsafe {
                        (g.api.x_sync)(g.dpy, 0);
                    }
                    return Ok(ActionResult { method: "linux_xtest_type".into() });
                }
                Err(DriverError::new(
                    "input_failed",
                    "Text input requires wtype, xdotool, or X11 display",
                ))
            }
            ActionRequest::PressKey {
                bundle_id,
                key,
                modifiers,
            } => {
                let _ = activate_app(bundle_id);
                if let Some(g) = DisplayGuard::open() {
                    let sym = map_x11_keysym_name(key)
                        .map(str::to_string)
                        .unwrap_or_else(|| key.clone());
                    let mut m_syms = Vec::new();
                    for m in modifiers {
                        let ms = match m.to_ascii_lowercase().as_str() {
                            "ctrl" | "control" => "Control_L",
                            "alt" | "option" => "Alt_L",
                            "shift" => "Shift_L",
                            "super" | "win" | "cmd" | "command" | "meta" => "Super_L",
                            _ => return Err(DriverError::new("invalid_argument", format!("Unsupported modifier: {m}"))),
                        };
                        m_syms.push(ms);
                    }
                    for &ms in &m_syms {
                        send_xtest_key_sym(&g, ms, true);
                    }
                    if !send_xtest_key_sym(&g, &sym, true) {
                        for &ms in m_syms.iter().rev() {
                            send_xtest_key_sym(&g, ms, false);
                        }
                        return Err(DriverError::new("invalid_argument", format!("Unsupported X11 key: {key}")));
                    }
                    send_xtest_key_sym(&g, &sym, false);
                    for &ms in m_syms.iter().rev() {
                        send_xtest_key_sym(&g, ms, false);
                    }
                    unsafe {
                        (g.api.x_sync)(g.dpy, 0);
                    }
                    return Ok(ActionResult { method: "linux_press_key".into() });
                }
                if cmd_exists("wtype") && is_wayland_session() {
                    let mut args: Vec<String> = Vec::new();
                    for m in modifiers {
                        let wm = match m.to_ascii_lowercase().as_str() {
                            "ctrl" | "control" => "ctrl",
                            "alt" | "option" => "alt",
                            "shift" => "shift",
                            "super" | "win" | "cmd" | "command" | "meta" => "logo",
                            _ => return Err(DriverError::new("invalid_argument", format!("Unsupported modifier: {m}"))),
                        };
                        args.push("-M".into());
                        args.push(wm.into());
                    }
                    let sym = map_x11_keysym_name(key).unwrap_or(key);
                    args.push("-k".into());
                    args.push(sym.into());
                    let ok = Command::new("wtype")
                        .args(&args)
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if ok {
                        return Ok(ActionResult { method: "linux_wtype_key".into() });
                    }
                }
                Err(DriverError::new("input_failed", "Key press requires XTest or wtype"))
            }
            ActionRequest::Scroll {
                bundle_id,
                direction,
                amount,
                x,
                y,
            } => {
                let _ = activate_app(bundle_id);
                let g = DisplayGuard::open().ok_or_else(|| DriverError::new("input_failed", "No X11 display"))?;
                let motion = g.api.xtest_motion.ok_or_else(|| DriverError::new("input_failed", "libXtst unavailable"))?;
                let button = g.api.xtest_button.ok_or_else(|| DriverError::new("input_failed", "libXtst unavailable"))?;
                if let (Some(px), Some(py)) = (x, y) {
                    unsafe {
                        motion(g.dpy, -1, px.round() as c_int, py.round() as c_int, CURRENT_TIME);
                    }
                }
                let btn = match direction.as_str() {
                    "up" => 4,
                    "down" => 5,
                    "left" => 6,
                    "right" => 7,
                    _ => return Err(DriverError::new("invalid_argument", "Invalid scroll direction")),
                };
                let ticks = (*amount).clamp(1, 20);
                unsafe {
                    for _ in 0..ticks {
                        button(g.dpy, btn, 1, CURRENT_TIME);
                        button(g.dpy, btn, 0, CURRENT_TIME);
                    }
                    (g.api.x_sync)(g.dpy, 0);
                }
                Ok(ActionResult { method: "linux_scroll".into() })
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl OsDriver for LinuxDriver {
    fn doctor(&self) -> Result<DoctorStatus, DriverError> {
        Ok(native::doctor())
    }

    fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError> {
        native::list_apps()
    }

    fn activate_app(&self, bundle_id: &str) -> Result<(), DriverError> {
        native::activate_app(bundle_id)
    }

    fn launch_app(&self, bundle_id: &str) -> Result<(), DriverError> {
        native::launch_app(bundle_id)
    }

    fn inspect_app(
        &self,
        bundle_id: &str,
        window: Option<i32>,
        max_elements: i32,
        max_depth: i32,
    ) -> Result<AppSnapshot, DriverError> {
        native::inspect_app(bundle_id, window, max_elements, max_depth)
    }

    fn capture_and_ocr(
        &self,
        bundle_id: &str,
        window: Option<i32>,
        ocr_mode: OcrMode,
        encode_png: bool,
    ) -> Result<CaptureResult, DriverError> {
        native::capture_and_ocr(bundle_id, window, ocr_mode, encode_png)
    }

    fn render_som_png(
        &self,
        png_b64: &str,
        marks_json: &str,
        win_frame: &Rect,
    ) -> Result<String, DriverError> {
        render_som_rgba_to_b64(png_b64, marks_json, win_frame)
    }

    fn perform_action(&self, req: &ActionRequest) -> Result<ActionResult, DriverError> {
        native::perform_action(req)
    }

    fn open_url(&self, bundle_id: &str, url: &str) -> Result<(), DriverError> {
        native::open_url(bundle_id, url)
    }
}

#[cfg(not(target_os = "linux"))]
impl OsDriver for LinuxDriver {
    fn doctor(&self) -> Result<DoctorStatus, DriverError> {
        Ok(DoctorStatus {
            os: "linux".into(),
            accessibility: false,
            screen_recording: false,
            note: "Linux AT-SPI2/X11/Wayland driver active only on Linux builds.".into(),
        })
    }

    fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux driver requires Linux host."))
    }

    fn activate_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux driver requires Linux host."))
    }

    fn launch_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux driver requires Linux host."))
    }

    fn inspect_app(
        &self,
        _bundle_id: &str,
        _window: Option<i32>,
        _max_elements: i32,
        _max_depth: i32,
    ) -> Result<AppSnapshot, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux driver requires Linux host."))
    }

    fn capture_and_ocr(
        &self,
        _bundle_id: &str,
        _window: Option<i32>,
        _ocr_mode: OcrMode,
        _encode_png: bool,
    ) -> Result<CaptureResult, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux driver requires Linux host."))
    }

    fn render_som_png(
        &self,
        png_b64: &str,
        marks_json: &str,
        win_frame: &Rect,
    ) -> Result<String, DriverError> {
        render_som_rgba_to_b64(png_b64, marks_json, win_frame)
    }

    fn perform_action(&self, _req: &ActionRequest) -> Result<ActionResult, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Linux driver requires Linux host."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_x11_bgrx_and_parses_atspi_json() {
        // 2x1 BGRX pixel buffer (little-endian: B, G, R, X)
        let raw = [50u8, 120, 220, 0, 10, 20, 30, 0];
        let rgba = extract_ximage_rgba(&raw, 2, 1, 8, 32, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF);
        assert_eq!(rgba, vec![220, 120, 50, 255, 30, 20, 10, 255]);

        let sample = r#"{
            "ok": true,
            "app": "firefox",
            "frontmost_app": "firefox",
            "pid": 2048,
            "frontmost": true,
            "windows": [{"index":0,"title":"Mozilla Firefox","x":0,"y":0,"w":1280,"h":720}],
            "elements": [
                {"role":"push button","label":"Back","window":0,"depth":1,"enabled":true,"focused":false,"is_password":false,"offscreen":false,"actions":["click"],"x":12,"y":40,"w":30,"h":30},
                {"role":"password text","label":"Pass","value":"secret","window":0,"depth":2,"enabled":true,"focused":true,"is_password":true,"offscreen":false,"actions":[],"x":100,"y":100,"w":200,"h":28}
            ]
        }"#;
        let snap = parse_atspi_snapshot_json(sample, "firefox", None, 100, 10).unwrap();
        assert_eq!(snap.bundle_id, "firefox");
        assert!(snap.has_secure_field);
        assert_eq!(snap.elements[0].role, "Button");
        assert_eq!(snap.elements[0].ref_id, "e1");
        assert_eq!(snap.elements[1].role, "SecureTextField");
        assert_eq!(snap.elements[1].value, None);
    }

    #[test]
    fn maps_x11_special_keys() {
        assert_eq!(map_x11_keysym_name("enter"), Some("Return"));
        assert_eq!(map_x11_keysym_name("/"), Some("slash"));
        assert_eq!(map_x11_keysym_name("pagedown"), Some("Next"));
    }
}
