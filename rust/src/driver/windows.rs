#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use super::{
    ActionRequest, ActionResult, AppSnapshot, CaptureResult, DoctorStatus, DriverError, OcrBox,
    OcrMode, OsDriver, Rect, RunningApp,
};
use crate::som_png::render_som_rgba_to_b64;

/// Maps Microsoft UI Automation `UIA_ControlTypeId` (50000..=50040) to ApexUse role strings.
pub fn uia_control_type_name(ctrl_type: i32) -> &'static str {
    match ctrl_type {
        50000 | 50031 => "Button",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 | 50016 => "TextField",
        50005 => "Link",
        50006 => "Image",
        50007 | 50029 => "ListItem",
        50008 => "List",
        50009 | 50010 => "Menu",
        50011 => "MenuItem",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50018 | 50019 => "Tab",
        50020 | 50017 | 50022 | 50034 | 50035 | 50037 => "StaticText",
        50021 => "ToolBar",
        50023 => "Tree",
        50024 => "TreeItem",
        50026 | 50033 => "Group",
        50028 | 50036 => "Table",
        50030 => "TextArea",
        50032 => "Window",
        _ => "Element",
    }
}

/// Normalizes global virtual-desktop screen coordinates `(x, y)` into 0..=65535 `MOUSEEVENTF_VIRTUALDESK` space.
pub fn normalize_virtual_desktop_pt(
    x: f64,
    y: f64,
    vx: i32,
    vy: i32,
    vw: i32,
    vh: i32,
) -> (i32, i32) {
    let w = vw.max(1) as f64;
    let h = vh.max(1) as f64;
    let nx = (((x - vx as f64) * 65535.0) / (w - 1.0).max(1.0))
        .round()
        .clamp(0.0, 65535.0) as i32;
    let ny = (((y - vy as f64) * 65535.0) / (h - 1.0).max(1.0))
        .round()
        .clamp(0.0, 65535.0) as i32;
    (nx, ny)
}

pub fn map_key_vk(key: &str) -> Option<u16> {
    let lower = key.trim().to_ascii_lowercase();
    let vk = match lower.as_str() {
        "return" | "enter" => 0x0D,
        "tab" => 0x09,
        "space" => 0x20,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" | "forwarddelete" => 0x2E,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
        "f13" => 0x7C,
        "f14" => 0x7D,
        "f15" => 0x7E,
        "f16" => 0x7F,
        "f17" => 0x80,
        "f18" => 0x81,
        "f19" => 0x82,
        "f20" => 0x83,
        _ if lower.len() == 1 => {
            let b = lower.as_bytes()[0];
            match b {
                b'a'..=b'z' => b.to_ascii_uppercase() as u16,
                b'0'..=b'9' => b as u16,
                b'-' => 0xBD,
                b'=' => 0xBB,
                b'[' => 0xDB,
                b']' => 0xDD,
                b';' => 0xBA,
                b'\'' => 0xDE,
                b'\\' => 0xDC,
                b',' => 0xBC,
                b'.' => 0xBE,
                b'/' => 0xBF,
                b'`' => 0xC0,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(vk)
}

pub fn map_modifier_vk(mod_name: &str) -> Option<u16> {
    match mod_name.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(0x11),
        "alt" | "option" => Some(0x12),
        "shift" => Some(0x10),
        "win" | "super" | "cmd" | "command" | "meta" => Some(0x5B),
        _ => None,
    }
}

pub fn parse_win_ocr_lines(output: &str, origin_x: f64, origin_y: f64) -> Vec<OcrBox> {
    let mut boxes = Vec::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 5 {
            continue;
        }
        let text = cols[4].trim();
        if text.is_empty() {
            continue;
        }
        let (Ok(x), Ok(y), Ok(w), Ok(h)) = (
            cols[0].trim().parse::<f64>(),
            cols[1].trim().parse::<f64>(),
            cols[2].trim().parse::<f64>(),
            cols[3].trim().parse::<f64>(),
        ) else {
            continue;
        };
        if w < 2.0 || h < 2.0 {
            continue;
        }
        boxes.push(OcrBox {
            text: text.to_string(),
            confidence: 0.90,
            frame: Rect {
                x: origin_x + x,
                y: origin_y + y,
                w,
                h,
            },
        });
    }
    boxes
}

/// Generates a premultiplied BGRA 32-bit DIB bitmap for the floating `AgentCursor` overlay.
pub fn build_cursor_argb_bitmap(size: usize, is_click: bool) -> Vec<u32> {
    let mut pixels = vec![0u32; size * size];
    let cx = (size / 2) as f64;
    let cy = (size / 2) as f64;

    if is_click {
        for y in 0..size {
            for x in 0..size {
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                let r = (dx * dx + dy * dy).sqrt();
                if (12.0..=24.0).contains(&r) {
                    let dist = (r - 18.0).abs();
                    if dist <= 3.0 {
                        let a = ((1.0 - dist / 3.0) * 180.0).round() as u32;
                        let pr = (15 * a) / 255;
                        let pg = (199 * a) / 255;
                        let pb = (166 * a) / 255;
                        pixels[y * size + x] = (a << 24) | (pr << 16) | (pg << 8) | pb;
                    }
                }
            }
        }
    }

    // Draw crisp arrow pointer with hotspot centered around (cx, cy).
    let ox = size / 2;
    let oy = size / 2;
    for row in 0..20usize {
        let max_col = if row < 14 { row + 1 } else { (19 - row).max(4) };
        for col in 0..=max_col {
            let px = ox + col;
            let py = oy + row;
            if px >= size || py >= size {
                continue;
            }
            let is_edge = col == 0 || col == max_col || row == 19 || (row >= 14 && col >= 4 && row == 14);
            let col_bgra = if is_edge {
                0xFF_FF_FF_FFu32
            } else {
                0xFF_0F_C7_A6u32
            };
            pixels[py * size + px] = col_bgra;
        }
    }
    pixels
}

pub struct WindowsDriver;

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use crate::driver::common::{alias_candidates, app_matches, build_snapshot_from_raw, RawElement};
    use crate::driver::WindowMeta;
    use crate::som_png::{cache_rgba_frame, compute_pixel_digest, encode_rgba_png_b64};
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use std::sync::{mpsc, Once, OnceLock};
    use std::time::{Duration, Instant};

    type Bool = i32;
    type HResult = i32;
    type Dword = u32;
    type Handle = *mut c_void;
    type Hwnd = *mut c_void;
    type Hdc = *mut c_void;
    type Hbitmap = *mut c_void;
    type Hgdiobj = *mut c_void;
    type Bstr = *mut u16;

    const S_OK: HResult = 0;
    const COINIT_APARTMENTTHREADED: Dword = 2;
    const CLSCTX_INPROC_SERVER: Dword = 1;
    const PROCESS_QUERY_LIMITED_INFORMATION: Dword = 0x1000;
    const DWMWA_EXTENDED_FRAME_BOUNDS: Dword = 9;
    const DWMWA_CLOAKED: Dword = 14;
    const SW_RESTORE: i32 = 9;
    const SW_SHOWNOACTIVATE: i32 = 4;
    const SM_XVIRTUALSCREEN: i32 = 76;
    const SM_YVIRTUALSCREEN: i32 = 77;
    const SM_CXVIRTUALSCREEN: i32 = 78;
    const SM_CYVIRTUALSCREEN: i32 = 79;
    const SRCCOPY: Dword = 0x00CC0020;
    const CAPTUREBLT: Dword = 0x40000000;
    const PW_RENDERFULLCONTENT: Dword = 0x00000002;
    const DIB_RGB_COLORS: Dword = 0;
    const BI_RGB: Dword = 0;

    const INPUT_MOUSE: Dword = 0;
    const INPUT_KEYBOARD: Dword = 1;
    const MOUSEEVENTF_MOVE: Dword = 0x0001;
    const MOUSEEVENTF_LEFTDOWN: Dword = 0x0002;
    const MOUSEEVENTF_LEFTUP: Dword = 0x0004;
    const MOUSEEVENTF_RIGHTDOWN: Dword = 0x0008;
    const MOUSEEVENTF_RIGHTUP: Dword = 0x0010;
    const MOUSEEVENTF_WHEEL: Dword = 0x0800;
    const MOUSEEVENTF_HWHEEL: Dword = 0x1000;
    const MOUSEEVENTF_VIRTUALDESK: Dword = 0x4000;
    const MOUSEEVENTF_ABSOLUTE: Dword = 0x8000;

    const KEYEVENTF_KEYUP: Dword = 0x0002;
    const KEYEVENTF_UNICODE: Dword = 0x0004;

    const UIA_INVOKE_PATTERN_ID: i32 = 10000;
    const UIA_VALUE_PATTERN_ID: i32 = 10002;

    const WS_POPUP: Dword = 0x80000000;
    const WS_EX_TOPMOST: Dword = 0x00000008;
    const WS_EX_TRANSPARENT: Dword = 0x00000020;
    const WS_EX_TOOLWINDOW: Dword = 0x00000080;
    const WS_EX_LAYERED: Dword = 0x00080000;
    const WS_EX_NOACTIVATE: Dword = 0x08000000;
    const ULW_ALPHA: Dword = 0x00000002;
    const AC_SRC_OVER: u8 = 0;
    const AC_SRC_ALPHA: u8 = 1;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const CLSID_CUI_AUTOMATION: Guid = Guid {
        data1: 0xFF48DBA4,
        data2: 0x60EF,
        data3: 0x4201,
        data4: [0xAA, 0x87, 0x54, 0x10, 0x3E, 0xEF, 0x59, 0x4E],
    };

    const IID_IUI_AUTOMATION: Guid = Guid {
        data1: 0x30CBE57D,
        data2: 0xD9D0,
        data3: 0x452A,
        data4: [0xAB, 0x13, 0x7A, 0xC5, 0xAC, 0x48, 0x25, 0xEE],
    };

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Size {
        cx: i32,
        cy: i32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct BlendFunction {
        blend_op: u8,
        blend_flags: u8,
        source_constant_alpha: u8,
        alpha_format: u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct BitmapInfoHeader {
        bi_size: Dword,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: Dword,
        bi_size_image: Dword,
        bi_x_pels_per_meter: i32,
        bi_y_pels_per_meter: i32,
        bi_clr_used: Dword,
        bi_clr_important: Dword,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct BitmapInfo {
        bmi_header: BitmapInfoHeader,
        bmi_colors: [Dword; 1],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MouseInput {
        dx: i32,
        dy: i32,
        mouse_data: Dword,
        dw_flags: Dword,
        time: Dword,
        dw_extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeybdInput {
        w_vk: u16,
        w_scan: u16,
        dw_flags: Dword,
        time: Dword,
        dw_extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct HardwareInput {
        u_msg: Dword,
        w_param_l: u16,
        w_param_h: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    union InputUnion {
        mi: MouseInput,
        ki: KeybdInput,
        hi: HardwareInput,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Input {
        r#type: Dword,
        u: InputUnion,
    }

    #[repr(C)]
    struct WndClassExW {
        cb_size: Dword,
        style: Dword,
        lpfn_wnd_proc: extern "system" fn(Hwnd, u32, usize, isize) -> isize,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: Handle,
        h_icon: Handle,
        h_cursor: Handle,
        hbr_background: Handle,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: Handle,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: Dword,
        pt: Point,
        l_private: Dword,
    }

    // Verified COM vtable structs matching UIAutomationClient.h sequential MIDL slots.
    #[repr(C)]
    struct IUnknownVtbl {
        query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    #[repr(C)]
    struct IUIAutomation {
        vtbl: *const IUIAutomationVtbl,
    }

    #[repr(C)]
    struct IUIAutomationVtbl {
        base: IUnknownVtbl,
        compare_elements: *const c_void,
        compare_runtime_ids: *const c_void,
        get_root_element: unsafe extern "system" fn(*mut IUIAutomation, *mut *mut IUIAutomationElement) -> HResult,
        element_from_handle: unsafe extern "system" fn(*mut IUIAutomation, Hwnd, *mut *mut IUIAutomationElement) -> HResult,
        element_from_point: unsafe extern "system" fn(*mut IUIAutomation, Point, *mut *mut IUIAutomationElement) -> HResult,
        get_focused_element: unsafe extern "system" fn(*mut IUIAutomation, *mut *mut IUIAutomationElement) -> HResult,
        get_root_element_build_cache: *const c_void,
        element_from_handle_build_cache: *const c_void,
        element_from_point_build_cache: *const c_void,
        get_focused_element_build_cache: *const c_void,
        create_tree_walker: *const c_void,
        get_control_view_walker: unsafe extern "system" fn(*mut IUIAutomation, *mut *mut IUIAutomationTreeWalker) -> HResult,
    }

    #[repr(C)]
    struct IUIAutomationTreeWalker {
        vtbl: *const IUIAutomationTreeWalkerVtbl,
    }

    #[repr(C)]
    struct IUIAutomationTreeWalkerVtbl {
        base: IUnknownVtbl,
        get_parent_element: unsafe extern "system" fn(*mut IUIAutomationTreeWalker, *mut IUIAutomationElement, *mut *mut IUIAutomationElement) -> HResult,
        get_first_child_element: unsafe extern "system" fn(*mut IUIAutomationTreeWalker, *mut IUIAutomationElement, *mut *mut IUIAutomationElement) -> HResult,
        get_last_child_element: *const c_void,
        get_next_sibling_element: unsafe extern "system" fn(*mut IUIAutomationTreeWalker, *mut IUIAutomationElement, *mut *mut IUIAutomationElement) -> HResult,
    }

    #[repr(C)]
    struct IUIAutomationElement {
        vtbl: *const IUIAutomationElementVtbl,
    }

    #[repr(C)]
    struct IUIAutomationElementVtbl {
        base: IUnknownVtbl,
        set_focus: unsafe extern "system" fn(*mut IUIAutomationElement) -> HResult,
        get_runtime_id: *const c_void,
        find_first: *const c_void,
        find_all: *const c_void,
        find_first_build_cache: *const c_void,
        find_all_build_cache: *const c_void,
        build_updated_cache: *const c_void,
        get_current_property_value: *const c_void,
        get_current_property_value_ex: *const c_void,
        get_cached_property_value: *const c_void,
        get_cached_property_value_ex: *const c_void,
        get_current_pattern_as: *const c_void,
        get_cached_pattern_as: *const c_void,
        get_current_pattern: unsafe extern "system" fn(*mut IUIAutomationElement, i32, *mut *mut c_void) -> HResult,
        get_cached_pattern: *const c_void,
        get_cached_parent: *const c_void,
        get_cached_children: *const c_void,
        get_current_process_id: unsafe extern "system" fn(*mut IUIAutomationElement, *mut i32) -> HResult,
        get_current_control_type: unsafe extern "system" fn(*mut IUIAutomationElement, *mut i32) -> HResult,
        get_current_localized_control_type: *const c_void,
        get_current_name: unsafe extern "system" fn(*mut IUIAutomationElement, *mut Bstr) -> HResult,
        get_current_accelerator_key: *const c_void,
        get_current_access_key: *const c_void,
        get_current_has_keyboard_focus: unsafe extern "system" fn(*mut IUIAutomationElement, *mut Bool) -> HResult,
        get_current_is_keyboard_focusable: *const c_void,
        get_current_is_enabled: unsafe extern "system" fn(*mut IUIAutomationElement, *mut Bool) -> HResult,
        get_current_automation_id: *const c_void,
        get_current_class_name: *const c_void,
        get_current_help_text: *const c_void,
        get_current_culture: *const c_void,
        get_current_is_control_element: *const c_void,
        get_current_is_content_element: *const c_void,
        get_current_is_password: unsafe extern "system" fn(*mut IUIAutomationElement, *mut Bool) -> HResult,
        get_current_native_window_handle: *const c_void,
        get_current_item_type: *const c_void,
        get_current_is_offscreen: unsafe extern "system" fn(*mut IUIAutomationElement, *mut Bool) -> HResult,
        get_current_orientation: *const c_void,
        get_current_framework_id: *const c_void,
        get_current_is_required_for_form: *const c_void,
        get_current_item_status: *const c_void,
        get_current_bounding_rectangle: unsafe extern "system" fn(*mut IUIAutomationElement, *mut WinRect) -> HResult,
    }

    #[repr(C)]
    struct IUIAutomationInvokePattern {
        vtbl: *const IUIAutomationInvokePatternVtbl,
    }

    #[repr(C)]
    struct IUIAutomationInvokePatternVtbl {
        base: IUnknownVtbl,
        invoke: unsafe extern "system" fn(*mut IUIAutomationInvokePattern) -> HResult,
    }

    #[repr(C)]
    struct IUIAutomationValuePattern {
        vtbl: *const IUIAutomationValuePatternVtbl,
    }

    #[repr(C)]
    struct IUIAutomationValuePatternVtbl {
        base: IUnknownVtbl,
        set_value: unsafe extern "system" fn(*mut IUIAutomationValuePattern, Bstr) -> HResult,
        get_current_value: unsafe extern "system" fn(*mut IUIAutomationValuePattern, *mut Bstr) -> HResult,
        get_current_is_read_only: *const c_void,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: Dword, inherit: Bool, pid: Dword) -> Handle;
        fn CloseHandle(h: Handle) -> Bool;
        fn QueryFullProcessImageNameW(process: Handle, flags: Dword, exe_name: *mut u16, size: *mut Dword) -> Bool;
        fn GetModuleHandleW(name: *const u16) -> Handle;
        fn GetProcAddress(module: Handle, proc_name: *const i8) -> *const c_void;
    }

    #[link(name = "user32")]
    extern "system" {
        fn SetProcessDPIAware() -> Bool;
        fn EnumWindows(cb: extern "system" fn(Hwnd, isize) -> Bool, lparam: isize) -> Bool;
        fn IsWindowVisible(hwnd: Hwnd) -> Bool;
        fn IsIconic(hwnd: Hwnd) -> Bool;
        fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut Dword) -> Dword;
        fn GetWindowTextW(hwnd: Hwnd, buf: *mut u16, max_count: i32) -> i32;
        fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        fn GetWindowRect(hwnd: Hwnd, rect: *mut WinRect) -> Bool;
        fn GetForegroundWindow() -> Hwnd;
        fn SetForegroundWindow(hwnd: Hwnd) -> Bool;
        fn BringWindowToTop(hwnd: Hwnd) -> Bool;
        fn ShowWindow(hwnd: Hwnd, cmd_show: i32) -> Bool;
        fn AllowSetForegroundWindow(pid: Dword) -> Bool;
        fn AttachThreadInput(id_attach: Dword, id_attach_to: Dword, attach: Bool) -> Bool;
        fn GetDC(hwnd: Hwnd) -> Hdc;
        fn ReleaseDC(hwnd: Hwnd, dc: Hdc) -> i32;
        fn PrintWindow(hwnd: Hwnd, hdc_blt: Hdc, flags: Dword) -> Bool;
        fn SendInput(c_inputs: Dword, p_inputs: *const Input, cb_size: i32) -> Dword;
        fn GetSystemMetrics(index: i32) -> i32;
        fn RegisterClassExW(wc: *const WndClassExW) -> u16;
        fn CreateWindowExW(
            ex_style: Dword,
            class_name: *const u16,
            window_name: *const u16,
            style: Dword,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: Hwnd,
            menu: Handle,
            instance: Handle,
            param: *mut c_void,
        ) -> Hwnd;
        fn UpdateLayeredWindow(
            hwnd: Hwnd,
            hdc_dst: Hdc,
            pt_dst: *const Point,
            psize: *const Size,
            hdc_src: Hdc,
            pt_src: *const Point,
            cr_key: Dword,
            pblend: *const BlendFunction,
            flags: Dword,
        ) -> Bool;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min_f: u32, max_f: u32, remove: u32) -> Bool;
        fn TranslateMessage(msg: *const Msg) -> Bool;
        fn DispatchMessageW(msg: *const Msg) -> isize;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
        fn DeleteDC(hdc: Hdc) -> Bool;
        fn CreateDIBSection(
            hdc: Hdc,
            pbmi: *const BitmapInfo,
            usage: Dword,
            ppv_bits: *mut *mut c_void,
            hsection: Handle,
            offset: Dword,
        ) -> Hbitmap;
        fn CreateCompatibleBitmap(hdc: Hdc, w: i32, h: i32) -> Hbitmap;
        fn SelectObject(hdc: Hdc, obj: Hgdiobj) -> Hgdiobj;
        fn DeleteObject(obj: Hgdiobj) -> Bool;
        fn BitBlt(
            hdc_dst: Hdc,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            hdc_src: Hdc,
            x1: i32,
            y1: i32,
            rop: Dword,
        ) -> Bool;
        fn GetDIBits(
            hdc: Hdc,
            hbm: Hbitmap,
            start: Dword,
            lines: Dword,
            bits: *mut c_void,
            lpbmi: *mut BitmapInfo,
            usage: Dword,
        ) -> i32;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetWindowAttribute(hwnd: Hwnd, attr: Dword, out: *mut c_void, size: Dword) -> HResult;
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, dw_co_init: Dword) -> HResult;
        fn CoCreateInstance(
            rclsid: *const Guid,
            outer: *mut c_void,
            dw_cls_ctx: Dword,
            riid: *const Guid,
            ppv: *mut *mut c_void,
        ) -> HResult;
    }

    #[link(name = "oleaut32")]
    extern "system" {
        fn SysAllocString(psz: *const u16) -> Bstr;
        fn SysStringLen(bstr: Bstr) -> Dword;
        fn SysFreeString(bstr: Bstr);
    }

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: Hwnd,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> Handle;
    }

    struct ComPtr<T>(*mut T);
    impl<T> ComPtr<T> {
        unsafe fn release_ref(&self) {
            if !self.0.is_null() {
                let unk = &**(self.0 as *mut *const IUnknownVtbl);
                (unk.release)(self.0 as *mut c_void);
            }
        }
    }
    impl<T> Drop for ComPtr<T> {
        fn drop(&mut self) {
            unsafe { self.release_ref() };
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn bstr_to_string_free(bstr: Bstr) -> String {
        if bstr.is_null() {
            return String::new();
        }
        let len = SysStringLen(bstr) as usize;
        let slice = std::slice::from_raw_parts(bstr, len);
        let out = String::from_utf16_lossy(slice);
        SysFreeString(bstr);
        out
    }

    pub fn init_dpi_and_com() {
        static DPI_ONCE: Once = Once::new();
        DPI_ONCE.call_once(|| unsafe {
            let user32_name = to_wide("user32.dll");
            let hmod = GetModuleHandleW(user32_name.as_ptr());
            let mut set = false;
            if !hmod.is_null() {
                let proc = GetProcAddress(
                    hmod,
                    b"SetProcessDpiAwarenessContext\0".as_ptr() as *const i8,
                );
                if !proc.is_null() {
                    let f: extern "system" fn(isize) -> Bool = std::mem::transmute(proc);
                    if f(-4isize) != 0 {
                        set = true;
                    }
                }
            }
            if !set {
                let _ = SetProcessDPIAware();
            }
        });
        unsafe {
            let _ = CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED);
        }
    }

    fn create_uia() -> Result<ComPtr<IUIAutomation>, DriverError> {
        init_dpi_and_com();
        let mut ptr: *mut c_void = null_mut();
        let hr = unsafe {
            CoCreateInstance(
                &CLSID_CUI_AUTOMATION,
                null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_IUI_AUTOMATION,
                &mut ptr,
            )
        };
        if hr != S_OK || ptr.is_null() {
            return Err(DriverError::new(
                "accessibility_unavailable",
                format!("UIAutomation COM init failed (hr=0x{hr:08x})"),
            ));
        }
        Ok(ComPtr(ptr as *mut IUIAutomation))
    }

    #[derive(Clone)]
    struct WinInfo {
        hwnd: Hwnd,
        pid: u32,
        exe_name: String,
        title: String,
        rect: Rect,
        active: bool,
    }

    fn get_process_exe_name(pid: u32) -> String {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return format!("pid:{pid}");
            }
            let mut buf = vec![0u16; 512];
            let mut len = buf.len() as Dword;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
            CloseHandle(h);
            if ok == 0 || len == 0 {
                return format!("pid:{pid}");
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string()
        }
    }

    fn window_bounds(hwnd: Hwnd) -> Option<Rect> {
        unsafe {
            let mut r = WinRect::default();
            let hr = DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut r as *mut WinRect as *mut c_void,
                size_of::<WinRect>() as Dword,
            );
            if hr != S_OK && GetWindowRect(hwnd, &mut r) == 0 {
                return None;
            }
            let w = (r.right - r.left) as f64;
            let h = (r.bottom - r.top) as f64;
            if w <= 1.0 || h <= 1.0 {
                return None;
            }
            Some(Rect {
                x: r.left as f64,
                y: r.top as f64,
                w,
                h,
            })
        }
    }

    extern "system" fn enum_win_cb(hwnd: Hwnd, lparam: isize) -> Bool {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut cloaked: Dword = 0;
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut Dword as *mut c_void,
                size_of::<Dword>() as Dword,
            ) == S_OK
                && cloaked != 0
            {
                return 1;
            }
            let Some(rect) = window_bounds(hwnd) else {
                return 1;
            };
            let title_len = GetWindowTextLengthW(hwnd);
            let mut title = String::new();
            if title_len > 0 {
                let mut tbuf = vec![0u16; (title_len + 1) as usize];
                let got = GetWindowTextW(hwnd, tbuf.as_mut_ptr(), title_len + 1);
                if got > 0 {
                    title = String::from_utf16_lossy(&tbuf[..got as usize]);
                }
            }
            if title.is_empty() {
                return 1;
            }
            let mut pid: Dword = 0;
            let _ = GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == 0 {
                return 1;
            }
            let list = &mut *(lparam as *mut Vec<WinInfo>);
            let fg = GetForegroundWindow();
            list.push(WinInfo {
                hwnd,
                pid,
                exe_name: get_process_exe_name(pid),
                title,
                rect,
                active: hwnd == fg,
            });
            1
        }
    }

    fn visible_windows() -> Vec<WinInfo> {
        init_dpi_and_com();
        let mut wins = Vec::new();
        unsafe {
            let _ = EnumWindows(enum_win_cb, &mut wins as *mut Vec<WinInfo> as isize);
        }
        wins
    }

    fn resolve_target_windows(bundle_id: &str) -> Result<(Vec<WinInfo>, String, i64, bool), DriverError> {
        let all = visible_windows();
        let fg_app = all
            .iter()
            .find(|w| w.active)
            .map(|w| w.exe_name.clone())
            .or_else(|| all.first().map(|w| w.exe_name.clone()))
            .unwrap_or_default();

        if bundle_id.eq_ignore_ascii_case("desktop") {
            let active_wins: Vec<WinInfo> = all
                .iter()
                .filter(|w| w.active)
                .cloned()
                .collect();
            let chosen = if active_wins.is_empty() { all } else { active_wins };
            let pid = chosen.first().map(|w| w.pid as i64).unwrap_or(0);
            return Ok((chosen, fg_app, pid, true));
        }

        let matched: Vec<WinInfo> = all
            .into_iter()
            .filter(|w| app_matches(bundle_id, &w.exe_name, None, Some(&w.title), w.pid as i64))
            .collect();
        if matched.is_empty() {
            return Err(DriverError::new(
                "app_not_running",
                format!("No visible Windows window matching '{bundle_id}'"),
            ));
        }
        let pid = matched[0].pid as i64;
        let is_fg = matched.iter().any(|w| w.active);
        let front_name = if fg_app.is_empty() {
            matched[0].exe_name.clone()
        } else {
            fg_app
        };
        Ok((matched, front_name, pid, is_fg))
    }

    pub fn doctor() -> DoctorStatus {
        let uia_ok = create_uia().is_ok();
        DoctorStatus {
            os: "windows".into(),
            accessibility: uia_ok,
            screen_recording: true,
            note: if uia_ok {
                "Windows UI Automation + GDI/WinRT OCR + SendInput active".into()
            } else {
                "Windows UI Automation COM unavailable".into()
            },
        }
    }

    pub fn list_apps() -> Vec<RunningApp> {
        let wins = visible_windows();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for w in wins {
            let key = w.exe_name.to_ascii_lowercase();
            if seen.insert(key) {
                out.push(RunningApp {
                    bundle_id: w.exe_name,
                    pid: w.pid as i64,
                    active: w.active,
                });
            }
        }
        out
    }

    pub fn activate_app(bundle_id: &str) -> Result<(), DriverError> {
        let (wins, _, _, _) = resolve_target_windows(bundle_id)?;
        let hwnd = wins[0].hwnd;
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = AllowSetForegroundWindow(wins[0].pid);
            let fg = GetForegroundWindow();
            let fg_tid = if !fg.is_null() {
                GetWindowThreadProcessId(fg, null_mut())
            } else {
                0
            };
            let target_tid = GetWindowThreadProcessId(hwnd, null_mut());
            if fg_tid != 0 && target_tid != 0 && fg_tid != target_tid {
                AttachThreadInput(fg_tid, target_tid, 1);
                let _ = SetForegroundWindow(hwnd);
                let _ = BringWindowToTop(hwnd);
                AttachThreadInput(fg_tid, target_tid, 0);
            } else {
                let _ = SetForegroundWindow(hwnd);
                let _ = BringWindowToTop(hwnd);
            }
        }
        Ok(())
    }

    pub fn launch_app(bundle_id: &str) -> Result<(), DriverError> {
        init_dpi_and_com();
        let cands = alias_candidates(bundle_id);
        let op = to_wide("open");
        for cand in cands {
            let target = to_wide(&cand);
            let res = unsafe { ShellExecuteW(null_mut(), op.as_ptr(), target.as_ptr(), null(), null(), SW_RESTORE) };
            if (res as usize) > 32 {
                return Ok(());
            }
        }
        Err(DriverError::new(
            "launch_failed",
            format!("ShellExecuteW could not launch '{bundle_id}'"),
        ))
    }

    pub fn open_url(bundle_id: &str, url: &str) -> Result<(), DriverError> {
        init_dpi_and_com();
        let op = to_wide("open");
        let wurl = to_wide(url);
        if !bundle_id.is_empty() && !bundle_id.eq_ignore_ascii_case("desktop") {
            for cand in alias_candidates(bundle_id) {
                if cand.ends_with(".exe") || matches!(cand.as_str(), "chrome" | "msedge" | "firefox") {
                    let wexe = to_wide(&cand);
                    let res = unsafe { ShellExecuteW(null_mut(), op.as_ptr(), wexe.as_ptr(), wurl.as_ptr(), null(), SW_RESTORE) };
                    if (res as usize) > 32 {
                        return Ok(());
                    }
                }
            }
        }
        let res = unsafe { ShellExecuteW(null_mut(), op.as_ptr(), wurl.as_ptr(), null(), null(), SW_RESTORE) };
        if (res as usize) > 32 {
            Ok(())
        } else {
            Err(DriverError::new("launch_failed", "Failed opening URL via ShellExecuteW"))
        }
    }

    unsafe fn walk_uia_tree(
        walker: *mut IUIAutomationTreeWalker,
        el: *mut IUIAutomationElement,
        win_idx: usize,
        depth: usize,
        max_depth: i32,
        max_elements: i32,
        out: &mut Vec<RawElement>,
    ) {
        if el.is_null() || out.len() >= (max_elements.max(1) as usize) + 64 || depth > (max_depth.max(1) as usize) + 1 {
            return;
        }
        let vtbl = &*(*el).vtbl;
        let mut ctrl_type: i32 = 0;
        let _ = (vtbl.get_current_control_type)(el, &mut ctrl_type);
        let mut name_bstr: Bstr = null_mut();
        let _ = (vtbl.get_current_name)(el, &mut name_bstr);
        let label = bstr_to_string_free(name_bstr);
        let mut enabled_b: Bool = 1;
        let _ = (vtbl.get_current_is_enabled)(el, &mut enabled_b);
        let mut focus_b: Bool = 0;
        let _ = (vtbl.get_current_has_keyboard_focus)(el, &mut focus_b);
        let mut pwd_b: Bool = 0;
        let _ = (vtbl.get_current_is_password)(el, &mut pwd_b);
        let mut off_b: Bool = 0;
        let _ = (vtbl.get_current_is_offscreen)(el, &mut off_b);
        let mut wr = WinRect::default();
        let got_rect = (vtbl.get_current_bounding_rectangle)(el, &mut wr) == S_OK;
        let frame = if got_rect && wr.right > wr.left && wr.bottom > wr.top {
            Some(Rect {
                x: wr.left as f64,
                y: wr.top as f64,
                w: (wr.right - wr.left) as f64,
                h: (wr.bottom - wr.top) as f64,
            })
        } else {
            None
        };

        let mut value = None;
        if pwd_b == 0 && matches!(ctrl_type, 50003 | 50004 | 50030) {
            let mut pat_ptr: *mut c_void = null_mut();
            if (vtbl.get_current_pattern)(el, UIA_VALUE_PATTERN_ID, &mut pat_ptr) == S_OK && !pat_ptr.is_null() {
                let vpat = pat_ptr as *mut IUIAutomationValuePattern;
                let mut val_bstr: Bstr = null_mut();
                if ((*(*vpat).vtbl).get_current_value)(vpat, &mut val_bstr) == S_OK {
                    let s = bstr_to_string_free(val_bstr);
                    if !s.is_empty() {
                        value = Some(s);
                    }
                }
                let _ = ComPtr(vpat);
            }
        }

        out.push(RawElement {
            role: uia_control_type_name(ctrl_type).to_string(),
            label,
            value,
            window: win_idx,
            depth,
            enabled: Some(enabled_b != 0),
            focused: focus_b != 0,
            is_password: pwd_b != 0,
            offscreen: off_b != 0,
            actions: vec![],
            frame,
        });

        let wvtbl = &*(*walker).vtbl;
        let mut child: *mut IUIAutomationElement = null_mut();
        if (wvtbl.get_first_child_element)(walker, el, &mut child) == S_OK && !child.is_null() {
            let mut cur = child;
            while !cur.is_null() && out.len() < (max_elements.max(1) as usize) + 64 {
                let _holder = ComPtr(cur);
                walk_uia_tree(walker, cur, win_idx, depth + 1, max_depth, max_elements, out);
                let mut next: *mut IUIAutomationElement = null_mut();
                if (wvtbl.get_next_sibling_element)(walker, cur, &mut next) != S_OK {
                    break;
                }
                cur = next;
            }
        }
    }

    pub fn inspect_app(
        bundle_id: &str,
        window: Option<i32>,
        max_elements: i32,
        max_depth: i32,
    ) -> Result<AppSnapshot, DriverError> {
        let uia = create_uia()?;
        let (wins, fg_app, pid, is_fg) = resolve_target_windows(bundle_id)?;
        let mut walker_ptr: *mut IUIAutomationTreeWalker = null_mut();
        let hr = unsafe { ((*(*uia.0).vtbl).get_control_view_walker)(uia.0, &mut walker_ptr) };
        if hr != S_OK || walker_ptr.is_null() {
            return Err(DriverError::new("uia_walker_failed", "Failed getting UIA ControlViewWalker"));
        }
        let walker = ComPtr(walker_ptr);

        let selected: Vec<(usize, &WinInfo)> = match window {
            Some(idx) => {
                let u = idx as usize;
                let w = wins
                    .get(u)
                    .ok_or_else(|| DriverError::new("window_out_of_range", "Window index out of range"))?;
                vec![(u, w)]
            }
            None => wins.iter().enumerate().collect(),
        };

        let metas: Vec<WindowMeta> = wins
            .iter()
            .enumerate()
            .map(|(i, w)| WindowMeta {
                index: i,
                title: w.title.clone(),
                frame: Some(w.rect.clone()),
            })
            .collect();

        let mut raw = Vec::new();
        for (win_idx, w) in selected {
            let mut el_ptr: *mut IUIAutomationElement = null_mut();
            let ehr = unsafe { ((*(*uia.0).vtbl).element_from_handle)(uia.0, w.hwnd, &mut el_ptr) };
            if ehr == S_OK && !el_ptr.is_null() {
                let el = ComPtr(el_ptr);
                unsafe {
                    walk_uia_tree(walker.0, el.0, win_idx, 1, max_depth, max_elements, &mut raw);
                }
            }
        }

        let resolved_name = wins
            .first()
            .map(|w| w.exe_name.clone())
            .unwrap_or_else(|| bundle_id.to_string());
        Ok(build_snapshot_from_raw(
            resolved_name,
            fg_app,
            pid,
            is_fg,
            metas,
            raw,
            max_elements,
            max_depth,
        ))
    }

    fn capture_rect_rgba(hwnd: Option<Hwnd>, rect: &Rect) -> Result<(usize, usize, Vec<u8>), DriverError> {
        init_dpi_and_com();
        let w = rect.w.round().max(1.0) as usize;
        let h = rect.h.round().max(1.0) as usize;
        unsafe {
            let screen_dc = GetDC(null_mut());
            if screen_dc.is_null() {
                return Err(DriverError::new("capture_failed", "GetDC failed"));
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            let bmp = CreateCompatibleBitmap(screen_dc, w as i32, h as i32);
            if mem_dc.is_null() || bmp.is_null() {
                if !bmp.is_null() {
                    DeleteObject(bmp);
                }
                if !mem_dc.is_null() {
                    DeleteDC(mem_dc);
                }
                ReleaseDC(null_mut(), screen_dc);
                return Err(DriverError::new("capture_failed", "Failed creating compatible bitmap"));
            }
            let old = SelectObject(mem_dc, bmp);

            let mut captured = false;
            if let Some(hw) = hwnd {
                captured = PrintWindow(hw, mem_dc, PW_RENDERFULLCONTENT) != 0;
            }
            if !captured {
                captured = BitBlt(
                    mem_dc,
                    0,
                    0,
                    w as i32,
                    h as i32,
                    screen_dc,
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    SRCCOPY | CAPTUREBLT,
                ) != 0;
            }

            let mut bgra = vec![0u8; w * h * 4];
            let mut bi = BitmapInfo {
                bmi_header: BitmapInfoHeader {
                    bi_size: size_of::<BitmapInfoHeader>() as Dword,
                    bi_width: w as i32,
                    bi_height: -(h as i32),
                    bi_planes: 1,
                    bi_bit_count: 32,
                    bi_compression: BI_RGB,
                    bi_size_image: 0,
                    bi_x_pels_per_meter: 0,
                    bi_y_pels_per_meter: 0,
                    bi_clr_used: 0,
                    bi_clr_important: 0,
                },
                bmi_colors: [0],
            };
            let lines = GetDIBits(
                mem_dc,
                bmp,
                0,
                h as Dword,
                bgra.as_mut_ptr() as *mut c_void,
                &mut bi,
                DIB_RGB_COLORS,
            );

            // If PrintWindow yielded black/zero pixels, retry via screen BitBlt.
            if captured && bgra.iter().all(|&b| b == 0) && hwnd.is_some() {
                let _ = BitBlt(
                    mem_dc,
                    0,
                    0,
                    w as i32,
                    h as i32,
                    screen_dc,
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    SRCCOPY | CAPTUREBLT,
                );
                let _ = GetDIBits(
                    mem_dc,
                    bmp,
                    0,
                    h as Dword,
                    bgra.as_mut_ptr() as *mut c_void,
                    &mut bi,
                    DIB_RGB_COLORS,
                );
            }

            SelectObject(mem_dc, old);
            DeleteObject(bmp);
            DeleteDC(mem_dc);
            ReleaseDC(null_mut(), screen_dc);

            if !captured || lines == 0 {
                return Err(DriverError::new("capture_failed", "GDI screen capture failed"));
            }

            for px in bgra.chunks_exact_mut(4) {
                px.swap(0, 2);
                px[3] = 255;
            }
            Ok((w, h, bgra))
        }
    }

    fn run_winrt_ocr(w: usize, h: usize, rgba: &[u8], origin_x: f64, origin_y: f64) -> Vec<OcrBox> {
        let png_bytes = crate::som_png::encode_rgba_png_bytes(w, h, rgba);
        if png_bytes.is_empty() {
            return Vec::new();
        }
        let tmp = std::env::temp_dir().join(format!(
            "apexuse_ocr_{}_{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        if std::fs::write(&tmp, &png_bytes).is_err() {
            return Vec::new();
        }
        let script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrEngine, Windows.Media.Ocr, ContentType=WindowsRuntime]
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.IsGenericMethod })[0]
function Await($op, $type) {
  $m = $asTask.MakeGenericMethod($type)
  $t = $m.Invoke($null, @($op))
  return $t.GetAwaiter().GetResult()
}
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($env:APEXUSE_OCR_FILE)) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$dec = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bmp = Await ($dec.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$eng = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
$res = Await ($eng.RecognizeAsync($bmp)) ([Windows.Media.Ocr.OcrResult])
foreach ($line in $res.Lines) {
  foreach ($word in $line.Words) {
    $r = $word.BoundingRect
    $t = $word.Text -replace "[\t\r\n]+", " "
    [Console]::Out.WriteLine("$($r.X)`t$($r.Y)`t$($r.Width)`t$($r.Height)`t$t")
  }
}
"#;
        let out = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                script,
            ])
            .env("APEXUSE_OCR_FILE", &tmp)
            .output();
        let _ = std::fs::remove_file(&tmp);
        match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                parse_win_ocr_lines(&text, origin_x, origin_y)
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
        let (target_hwnd, win_frame, out_bundle, front_bundle, pid, wid) = if is_desktop && window.is_none() {
            let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f64;
            let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f64;
            let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1) as f64;
            let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1) as f64;
            let all = visible_windows();
            let fg = all.iter().find(|w| w.active).map(|w| w.exe_name.clone()).unwrap_or_default();
            (
                None,
                Rect {
                    x: vx,
                    y: vy,
                    w: vw,
                    h: vh,
                },
                "desktop".to_string(),
                fg,
                0,
                0u64,
            )
        } else {
            let (wins, fg, pid, _) = resolve_target_windows(bundle_id)?;
            let chosen = match window {
                Some(idx) => wins
                    .get(idx as usize)
                    .ok_or_else(|| DriverError::new("window_out_of_range", "Window index out of range"))?,
                None => wins
                    .iter()
                    .max_by(|a, b| (a.rect.w * a.rect.h).total_cmp(&(b.rect.w * b.rect.h)))
                    .unwrap(),
            };
            (
                Some(chosen.hwnd),
                chosen.rect.clone(),
                chosen.exe_name.clone(),
                fg,
                pid,
                chosen.hwnd as u64,
            )
        };

        let (w, h, rgba) = capture_rect_rgba(target_hwnd, &win_frame)?;
        let pixel_digest = compute_pixel_digest(w, h, &rgba);
        let ocr_boxes = if ocr_mode == OcrMode::Off {
            Vec::new()
        } else {
            run_winrt_ocr(w, h, &rgba, win_frame.x, win_frame.y)
        };
        let png_base64 = if encode_png {
            encode_rgba_png_b64(w, h, &rgba)
        } else {
            String::new()
        };
        let capture_id = cache_rgba_frame("win", w, h, rgba);

        Ok(CaptureResult {
            bundle_id: out_bundle,
            frontmost_bundle_id: front_bundle,
            pid,
            window_id: wid,
            capture_id: Some(capture_id),
            pixel_digest: Some(pixel_digest),
            window_frame: win_frame,
            image_width: w,
            image_height: h,
            png_base64,
            ocr_boxes,
        })
    }

    // MARK: - Windows Layered AgentCursor Overlay Thread

    struct CursorMove {
        x: i32,
        y: i32,
        click: bool,
    }

    static CURSOR_TX: OnceLock<mpsc::Sender<CursorMove>> = OnceLock::new();

    extern "system" fn cursor_wnd_proc(hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize {
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    fn cursor_sender() -> &'static mpsc::Sender<CursorMove> {
        CURSOR_TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<CursorMove>();
            std::thread::spawn(move || unsafe {
                let class_name = to_wide("ApexUseAgentCursor");
                let hinst = GetModuleHandleW(null());
                let wc = WndClassExW {
                    cb_size: size_of::<WndClassExW>() as Dword,
                    style: 0,
                    lpfn_wnd_proc: cursor_wnd_proc,
                    cb_cls_extra: 0,
                    cb_wnd_extra: 0,
                    h_instance: hinst,
                    h_icon: null_mut(),
                    h_cursor: null_mut(),
                    hbr_background: null_mut(),
                    lpsz_menu_name: null(),
                    lpsz_class_name: class_name.as_ptr(),
                    h_icon_sm: null_mut(),
                };
                RegisterClassExW(&wc);
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    64,
                    64,
                    null_mut(),
                    null_mut(),
                    hinst,
                    null_mut(),
                );
                let mut visible = false;
                let mut expires = Instant::now();
                loop {
                    let mut msg: Msg = zeroed();
                    while PeekMessageW(&mut msg, hwnd, 0, 0, 1) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    match rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(CursorMove { x, y, click }) => {
                            if !hwnd.is_null() {
                                let sz = 64usize;
                                let pixels = build_cursor_argb_bitmap(sz, click);
                                let scr = GetDC(null_mut());
                                let mem = CreateCompatibleDC(scr);
                                let bi = BitmapInfo {
                                    bmi_header: BitmapInfoHeader {
                                        bi_size: size_of::<BitmapInfoHeader>() as Dword,
                                        bi_width: sz as i32,
                                        bi_height: -(sz as i32),
                                        bi_planes: 1,
                                        bi_bit_count: 32,
                                        bi_compression: BI_RGB,
                                        bi_size_image: 0,
                                        bi_x_pels_per_meter: 0,
                                        bi_y_pels_per_meter: 0,
                                        bi_clr_used: 0,
                                        bi_clr_important: 0,
                                    },
                                    bmi_colors: [0],
                                };
                                let mut bits: *mut c_void = null_mut();
                                let dib = CreateDIBSection(scr, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                                if !dib.is_null() && !bits.is_null() {
                                    std::ptr::copy_nonoverlapping(
                                        pixels.as_ptr(),
                                        bits as *mut u32,
                                        pixels.len(),
                                    );
                                    let old = SelectObject(mem, dib);
                                    let dst = Point {
                                        x: x - (sz as i32) / 2,
                                        y: y - (sz as i32) / 2,
                                    };
                                    let wsz = Size {
                                        cx: sz as i32,
                                        cy: sz as i32,
                                    };
                                    let src_pt = Point { x: 0, y: 0 };
                                    let blend = BlendFunction {
                                        blend_op: AC_SRC_OVER,
                                        blend_flags: 0,
                                        source_constant_alpha: 255,
                                        alpha_format: AC_SRC_ALPHA,
                                    };
                                    UpdateLayeredWindow(hwnd, scr, &dst, &wsz, mem, &src_pt, 0, &blend, ULW_ALPHA);
                                    SelectObject(mem, old);
                                }
                                if !dib.is_null() {
                                    DeleteObject(dib);
                                }
                                DeleteDC(mem);
                                ReleaseDC(null_mut(), scr);
                                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                                visible = true;
                                expires = Instant::now() + Duration::from_secs(10);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if !hwnd.is_null() && visible && Instant::now() >= expires {
                                ShowWindow(hwnd, 0);
                                visible = false;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
            tx
        })
    }

    pub fn verify_point_safe(x: f64, y: f64) -> Result<bool, DriverError> {
        let Ok(uia) = create_uia() else {
            return Ok(true);
        };
        let mut el_ptr: *mut IUIAutomationElement = null_mut();
        let hr = unsafe {
            ((*(*uia.0).vtbl).element_from_point)(
                uia.0,
                Point {
                    x: x.round() as i32,
                    y: y.round() as i32,
                },
                &mut el_ptr,
            )
        };
        if hr != S_OK || el_ptr.is_null() {
            return Ok(true);
        }
        let el = ComPtr(el_ptr);
        let mut is_pwd: Bool = 0;
        let _ = unsafe { ((*(*el.0).vtbl).get_current_is_password)(el.0, &mut is_pwd) };
        Ok(is_pwd == 0)
    }

    fn send_mouse_abs(x: f64, y: f64, flags: Dword, data: Dword) {
        unsafe {
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let (nx, ny) = normalize_virtual_desktop_pt(x, y, vx, vy, vw, vh);
            let inp = Input {
                r#type: INPUT_MOUSE,
                u: InputUnion {
                    mi: MouseInput {
                        dx: nx,
                        dy: ny,
                        mouse_data: data,
                        dw_flags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dw_extra_info: 0,
                    },
                },
            };
            SendInput(1, &inp, size_of::<Input>() as i32);
        }
    }

    fn send_vk(vk: u16, up: bool) {
        let inp = Input {
            r#type: INPUT_KEYBOARD,
            u: InputUnion {
                ki: KeybdInput {
                    w_vk: vk,
                    w_scan: 0,
                    dw_flags: if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dw_extra_info: 0,
                },
            },
        };
        unsafe {
            SendInput(1, &inp, size_of::<Input>() as i32);
        }
    }

    fn push_vk_tap(inputs: &mut Vec<Input>, vk: u16) {
        inputs.push(Input {
            r#type: INPUT_KEYBOARD,
            u: InputUnion {
                ki: KeybdInput { w_vk: vk, w_scan: 0, dw_flags: 0, time: 0, dw_extra_info: 0 },
            },
        });
        inputs.push(Input {
            r#type: INPUT_KEYBOARD,
            u: InputUnion {
                ki: KeybdInput { w_vk: vk, w_scan: 0, dw_flags: KEYEVENTF_KEYUP, time: 0, dw_extra_info: 0 },
            },
        });
    }

    fn send_unicode_text(text: &str) {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let mut inputs = Vec::with_capacity(utf16.len() * 2);
        for code in utf16 {
            if code == b'\r' as u16 {
                continue;
            }
            if code == b'\n' as u16 {
                push_vk_tap(&mut inputs, 0x0D);
                continue;
            }
            if code == b'\t' as u16 {
                push_vk_tap(&mut inputs, 0x09);
                continue;
            }
            inputs.push(Input {
                r#type: INPUT_KEYBOARD,
                u: InputUnion {
                    ki: KeybdInput { w_vk: 0, w_scan: code, dw_flags: KEYEVENTF_UNICODE, time: 0, dw_extra_info: 0 },
                },
            });
            inputs.push(Input {
                r#type: INPUT_KEYBOARD,
                u: InputUnion {
                    ki: KeybdInput { w_vk: 0, w_scan: code, dw_flags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP, time: 0, dw_extra_info: 0 },
                },
            });
        }
        if !inputs.is_empty() {
            unsafe {
                SendInput(inputs.len() as Dword, inputs.as_ptr(), size_of::<Input>() as i32);
            }
        }
    }

    fn try_uia_invoke_at(x: f64, y: f64) -> bool {
        let Ok(uia) = create_uia() else {
            return false;
        };
        let mut el_ptr: *mut IUIAutomationElement = null_mut();
        let hr = unsafe {
            ((*(*uia.0).vtbl).element_from_point)(
                uia.0,
                Point {
                    x: x.round() as i32,
                    y: y.round() as i32,
                },
                &mut el_ptr,
            )
        };
        if hr != S_OK || el_ptr.is_null() {
            return false;
        }
        let el = ComPtr(el_ptr);
        let mut pat_ptr: *mut c_void = null_mut();
        if unsafe { ((*(*el.0).vtbl).get_current_pattern)(el.0, UIA_INVOKE_PATTERN_ID, &mut pat_ptr) } == S_OK
            && !pat_ptr.is_null()
        {
            let inv = ComPtr(pat_ptr as *mut IUIAutomationInvokePattern);
            return unsafe { ((*(*inv.0).vtbl).invoke)(inv.0) } == S_OK;
        }
        false
    }

    pub fn perform_action(req: &ActionRequest) -> Result<ActionResult, DriverError> {
        init_dpi_and_com();
        match req {
            ActionRequest::ClickPoint {
                bundle_id,
                x,
                y,
                prefer_ax,
                click_count,
                button,
            } => {
                if !verify_point_safe(*x, *y)? {
                    return Err(DriverError::new("unsafe_target_field", "Target point is a password control"));
                }
                let _ = activate_app(bundle_id);
                let _ = cursor_sender().send(CursorMove {
                    x: x.round() as i32,
                    y: y.round() as i32,
                    click: true,
                });
                if *prefer_ax && *click_count == 1 && button == "left" && try_uia_invoke_at(*x, *y) {
                    return Ok(ActionResult {
                        method: "windows_uia_invoke".into(),
                    });
                }
                let (down, up) = if button == "right" {
                    (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
                } else {
                    (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
                };
                send_mouse_abs(*x, *y, MOUSEEVENTF_MOVE, 0);
                for _ in 0..(*click_count).max(1) {
                    send_mouse_abs(*x, *y, down, 0);
                    send_mouse_abs(*x, *y, up, 0);
                }
                Ok(ActionResult {
                    method: "windows_sendinput".into(),
                })
            }
            ActionRequest::HoverPoint { x, y, .. } => {
                let _ = cursor_sender().send(CursorMove {
                    x: x.round() as i32,
                    y: y.round() as i32,
                    click: false,
                });
                send_mouse_abs(*x, *y, MOUSEEVENTF_MOVE, 0);
                Ok(ActionResult {
                    method: "windows_hover".into(),
                })
            }
            ActionRequest::DragPoints {
                bundle_id,
                x1,
                y1,
                x2,
                y2,
            } => {
                let _ = activate_app(bundle_id);
                let _ = cursor_sender().send(CursorMove {
                    x: x1.round() as i32,
                    y: y1.round() as i32,
                    click: true,
                });
                send_mouse_abs(*x1, *y1, MOUSEEVENTF_MOVE, 0);
                send_mouse_abs(*x1, *y1, MOUSEEVENTF_LEFTDOWN, 0);
                for step in 1..=8 {
                    let t = step as f64 / 8.0;
                    let ix = x1 + (x2 - x1) * t;
                    let iy = y1 + (y2 - y1) * t;
                    send_mouse_abs(ix, iy, MOUSEEVENTF_MOVE, 0);
                    std::thread::sleep(Duration::from_millis(12));
                }
                let _ = cursor_sender().send(CursorMove {
                    x: x2.round() as i32,
                    y: y2.round() as i32,
                    click: true,
                });
                send_mouse_abs(*x2, *y2, MOUSEEVENTF_LEFTUP, 0);
                Ok(ActionResult {
                    method: "windows_drag".into(),
                })
            }
            ActionRequest::TypePoint {
                bundle_id,
                x,
                y,
                text,
                mode,
                click_first,
            } => {
                if !verify_point_safe(*x, *y)? {
                    return Err(DriverError::new("unsafe_target_field", "Target point is a password control"));
                }
                let _ = activate_app(bundle_id);
                let _ = cursor_sender().send(CursorMove {
                    x: x.round() as i32,
                    y: y.round() as i32,
                    click: *click_first,
                });
                if *click_first {
                    send_mouse_abs(*x, *y, MOUSEEVENTF_MOVE, 0);
                    send_mouse_abs(*x, *y, MOUSEEVENTF_LEFTDOWN, 0);
                    send_mouse_abs(*x, *y, MOUSEEVENTF_LEFTUP, 0);
                    std::thread::sleep(Duration::from_millis(35));
                }
                if mode == "replace" {
                    // Try native UIA ValuePattern first, fallback to Ctrl+A + Backspace.
                    let mut used_value = false;
                    if let Ok(uia) = create_uia() {
                        let mut el_ptr: *mut IUIAutomationElement = null_mut();
                        if unsafe {
                            ((*(*uia.0).vtbl).element_from_point)(
                                uia.0,
                                Point {
                                    x: x.round() as i32,
                                    y: y.round() as i32,
                                },
                                &mut el_ptr,
                            )
                        } == S_OK
                            && !el_ptr.is_null()
                        {
                            let el = ComPtr(el_ptr);
                            let mut pat_ptr: *mut c_void = null_mut();
                            if unsafe {
                                ((*(*el.0).vtbl).get_current_pattern)(
                                    el.0,
                                    UIA_VALUE_PATTERN_ID,
                                    &mut pat_ptr,
                                )
                            } == S_OK
                                && !pat_ptr.is_null()
                            {
                                let vpat = ComPtr(pat_ptr as *mut IUIAutomationValuePattern);
                                let w = to_wide(text);
                                let b = unsafe { SysAllocString(w.as_ptr()) };
                                if !b.is_null() {
                                    if unsafe { ((*(*vpat.0).vtbl).set_value)(vpat.0, b) } == S_OK {
                                        used_value = true;
                                    }
                                    unsafe { SysFreeString(b) };
                                }
                            }
                        }
                    }
                    if used_value {
                        return Ok(ActionResult {
                            method: "windows_uia_value".into(),
                        });
                    }
                    send_vk(0x11, false); // VK_CONTROL
                    send_vk(b'A' as u16, false);
                    send_vk(b'A' as u16, true);
                    send_vk(0x11, true);
                    send_vk(0x08, false);
                    send_vk(0x08, true);
                    std::thread::sleep(Duration::from_millis(20));
                }
                send_unicode_text(text);
                Ok(ActionResult {
                    method: "windows_type".into(),
                })
            }
            ActionRequest::PressKey {
                bundle_id,
                key,
                modifiers,
            } => {
                let vk = map_key_vk(key)
                    .ok_or_else(|| DriverError::new("invalid_argument", format!("Unsupported key: {key}")))?;
                let mut m_vks = Vec::new();
                for m in modifiers {
                    let mv = map_modifier_vk(m)
                        .ok_or_else(|| DriverError::new("invalid_argument", format!("Unsupported modifier: {m}")))?;
                    m_vks.push(mv);
                }
                let _ = activate_app(bundle_id);
                for &mv in &m_vks {
                    send_vk(mv, false);
                }
                send_vk(vk, false);
                send_vk(vk, true);
                for &mv in m_vks.iter().rev() {
                    send_vk(mv, true);
                }
                Ok(ActionResult {
                    method: "windows_press_key".into(),
                })
            }
            ActionRequest::Scroll {
                bundle_id,
                direction,
                amount,
                x,
                y,
            } => {
                let (wins, _, _, _) = resolve_target_windows(bundle_id)?;
                let target_pt = match (x, y) {
                    (Some(px), Some(py)) => (*px, *py),
                    _ => {
                        let r = &wins[0].rect;
                        (r.x + r.w * 0.5, r.y + r.h * 0.5)
                    }
                };
                let _ = activate_app(bundle_id);
                let _ = cursor_sender().send(CursorMove {
                    x: target_pt.0.round() as i32,
                    y: target_pt.1.round() as i32,
                    click: false,
                });
                let step_delta = ((*amount).clamp(1, 20) * 120) as i32;
                let (flag, delta) = match direction.as_str() {
                    "up" => (MOUSEEVENTF_WHEEL, step_delta as Dword),
                    "down" => (MOUSEEVENTF_WHEEL, (-step_delta) as Dword),
                    "right" => (MOUSEEVENTF_HWHEEL, step_delta as Dword),
                    "left" => (MOUSEEVENTF_HWHEEL, (-step_delta) as Dword),
                    _ => return Err(DriverError::new("invalid_argument", "Invalid scroll direction")),
                };
                send_mouse_abs(target_pt.0, target_pt.1, MOUSEEVENTF_MOVE, 0);
                send_mouse_abs(target_pt.0, target_pt.1, flag, delta);
                Ok(ActionResult {
                    method: "windows_scroll".into(),
                })
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl OsDriver for WindowsDriver {
    fn doctor(&self) -> Result<DoctorStatus, DriverError> {
        Ok(native::doctor())
    }

    fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError> {
        Ok(native::list_apps())
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

    fn supports_fast_point_verify(&self) -> bool {
        true
    }

    fn verify_point_safe(&self, _bundle_id: &str, x: f64, y: f64) -> Result<bool, DriverError> {
        native::verify_point_safe(x, y)
    }

    fn open_url(&self, bundle_id: &str, url: &str) -> Result<(), DriverError> {
        native::open_url(bundle_id, url)
    }
}

#[cfg(not(target_os = "windows"))]
impl OsDriver for WindowsDriver {
    fn doctor(&self) -> Result<DoctorStatus, DriverError> {
        Ok(DoctorStatus {
            os: "windows".into(),
            accessibility: false,
            screen_recording: false,
            note: "Windows UIA/GDI driver active only on Windows builds.".into(),
        })
    }

    fn list_apps(&self) -> Result<Vec<RunningApp>, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows driver requires Windows host."))
    }

    fn activate_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows driver requires Windows host."))
    }

    fn launch_app(&self, _bundle_id: &str) -> Result<(), DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows driver requires Windows host."))
    }

    fn inspect_app(
        &self,
        _bundle_id: &str,
        _window: Option<i32>,
        _max_elements: i32,
        _max_depth: i32,
    ) -> Result<AppSnapshot, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows driver requires Windows host."))
    }

    fn capture_and_ocr(
        &self,
        _bundle_id: &str,
        _window: Option<i32>,
        _ocr_mode: OcrMode,
        _encode_png: bool,
    ) -> Result<CaptureResult, DriverError> {
        Err(DriverError::new("unsupported_on_platform", "Windows driver requires Windows host."))
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
        Err(DriverError::new("unsupported_on_platform", "Windows driver requires Windows host."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_key_role_and_virtual_desktop_mappings() {
        assert_eq!(uia_control_type_name(50000), "Button");
        assert_eq!(uia_control_type_name(50004), "TextField");
        assert_eq!(uia_control_type_name(50030), "TextArea");
        assert_eq!(map_key_vk("enter"), Some(0x0D));
        assert_eq!(map_key_vk("f12"), Some(0x7B));
        assert_eq!(map_key_vk("a"), Some(b'A' as u16));
        assert_eq!(map_modifier_vk("ctrl"), Some(0x11));

        let (nx, ny) = normalize_virtual_desktop_pt(0.0, 0.0, -1920, 0, 3840, 1080);
        assert!((32750..=32800).contains(&nx));
        assert_eq!(ny, 0);

        let ocr = parse_win_ocr_lines("120\t45\t80\t22\tBook\n0\t0\t1\t1\t.\n", 100.0, 200.0);
        assert_eq!(ocr.len(), 1);
        assert_eq!(ocr[0].text, "Book");
        assert!((ocr[0].frame.x - 220.0).abs() < 0.01);

        let bmp = build_cursor_argb_bitmap(64, true);
        assert_eq!(bmp.len(), 64 * 64);
        assert!(bmp.iter().any(|&p| (p >> 24) > 0));
    }
}
