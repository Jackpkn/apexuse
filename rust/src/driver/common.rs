use super::{AppSnapshot, OcrBox, Rect, UiElement, WindowMeta};
use crate::policy::contains_secure_marker;

/// Raw platform node collected by Windows UIA or Linux AT-SPI2 before
/// normalizing into an `AppSnapshot`.
#[derive(Debug, Clone)]
pub struct RawElement {
    pub role: String,
    pub label: String,
    pub value: Option<String>,
    pub window: usize,
    pub depth: usize,
    pub enabled: Option<bool>,
    pub focused: bool,
    pub is_password: bool,
    pub offscreen: bool,
    pub actions: Vec<String>,
    pub frame: Option<Rect>,
}

pub fn normalize_role(raw: &str) -> &'static str {
    let lower = raw.trim().to_ascii_lowercase();
    let clean = lower.strip_prefix("role_").unwrap_or(&lower);
    match clean {
        "button" | "push button" | "push-button" | "splitbutton" | "split button"
        | "toggle button" | "toggle-button" => "Button",
        "checkbox" | "check box" | "check-box" | "check menu item" => "CheckBox",
        "radiobutton" | "radio button" | "radio-button" | "radio menu item" => "RadioButton",
        "combobox" | "combo box" | "combo-box" => "ComboBox",
        "password text" | "password-text" | "securetextfield" => "SecureTextField",
        "search" | "searchbox" | "search field" => "SearchField",
        "edit" | "entry" | "text" | "textfield" | "text field" | "spin button" | "spinner" => {
            "TextField"
        }
        "document" | "document web" | "document-frame" | "textarea" | "text area" => "TextArea",
        "hyperlink" | "link" => "Link",
        "menuitem" | "menu item" | "menu-item" => "MenuItem",
        "menu" | "menubar" | "menu bar" | "menu-bar" | "popup menu" => "Menu",
        "tab" | "tabitem" | "tab item" | "page tab" | "page-tab" => "Tab",
        "listitem" | "list item" | "list-item" | "dataitem" | "table cell" => "ListItem",
        "treeitem" | "tree item" | "tree-item" => "TreeItem",
        "statictext" | "static text" | "label" | "heading" | "caption" => "StaticText",
        "window" | "frame" | "dialog" | "alert" => "Window",
        "scrollbar" | "scroll bar" | "scroll-bar" => "ScrollBar",
        "slider" | "scale" => "Slider",
        "image" | "icon" => "Image",
        "toolbar" | "tool bar" | "tool-bar" => "ToolBar",
        "pane" | "panel" | "group" | "grouping" | "filler" | "scroll pane" | "section" => "Group",
        "list" => "List",
        "tree" | "tree table" => "Tree",
        "table" | "datagrid" => "Table",
        "header" | "headeritem" | "titlebar" => "StaticText",
        _ => "Element",
    }
}

pub fn is_interactive_role(role: &str) -> bool {
    matches!(
        role,
        "Button"
            | "CheckBox"
            | "RadioButton"
            | "ComboBox"
            | "TextField"
            | "TextArea"
            | "SearchField"
            | "Link"
            | "MenuItem"
            | "Tab"
            | "ListItem"
            | "TreeItem"
            | "Slider"
    )
}

pub fn is_editable_role(role: &str) -> bool {
    matches!(
        role,
        "TextField" | "TextArea" | "SearchField" | "ComboBox"
    )
}

pub fn default_actions_for_role(role: &str) -> Vec<String> {
    match role {
        "Button" | "CheckBox" | "RadioButton" | "Link" | "MenuItem" | "Tab" | "ListItem"
        | "TreeItem" | "ComboBox" => vec!["press".into()],
        "TextField" | "TextArea" | "SearchField" => vec!["setValue".into(), "press".into()],
        _ => Vec::new(),
    }
}

pub fn build_snapshot_from_raw(
    bundle_id: String,
    frontmost_bundle_id: String,
    pid: i64,
    frontmost: bool,
    windows: Vec<WindowMeta>,
    raw_elements: Vec<RawElement>,
    max_elements: i32,
    max_depth: i32,
) -> AppSnapshot {
    let max_el = max_elements.max(1) as usize;
    let max_d = max_depth.max(1) as usize;
    let mut elements = Vec::new();
    let mut has_secure = false;
    let mut truncated = false;
    let mut depth_truncated = false;
    let mut ref_idx = 1usize;

    for raw in raw_elements {
        if raw.depth > max_d {
            depth_truncated = true;
            continue;
        }
        if raw.offscreen {
            continue;
        }
        let norm_role = if raw.is_password {
            has_secure = true;
            "SecureTextField"
        } else {
            normalize_role(&raw.role)
        };
        if contains_secure_marker(&raw.label)
            || raw.value.as_deref().map(contains_secure_marker).unwrap_or(false)
        {
            has_secure = true;
        }

        if elements.len() >= max_el {
            truncated = true;
            continue;
        }

        let interactive = !raw.is_password && is_interactive_role(norm_role);
        let ref_id = if interactive {
            let r = format!("e{ref_idx}");
            ref_idx += 1;
            r
        } else {
            String::new()
        };
        let actions = if raw.actions.is_empty() {
            default_actions_for_role(norm_role)
        } else {
            raw.actions
        };
        elements.push(UiElement {
            ref_id,
            role: norm_role.to_string(),
            label: if raw.is_password {
                "[secure]".into()
            } else {
                raw.label
            },
            value: if raw.is_password { None } else { raw.value },
            url: None,
            window: raw.window,
            depth: raw.depth,
            interactive,
            enabled: raw.enabled,
            focused: raw.focused,
            actions,
            frame: raw.frame,
        });
    }

    let windows_total = windows.len();
    AppSnapshot {
        bundle_id,
        frontmost_bundle_id,
        pid,
        frontmost,
        windows_total,
        windows,
        elements,
        has_secure_field: has_secure,
        has_unverified_field: false,
        truncated,
        depth_truncated,
        safety_scan_complete: true,
    }
}

/// Maps requested cross-OS bundle IDs / aliases to candidate process or desktop names.
pub fn alias_candidates(requested: &str) -> Vec<String> {
    let lower = requested.trim().to_ascii_lowercase();
    let clean = lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".desktop"))
        .unwrap_or(&lower);
    let mut out = vec![lower.clone(), clean.to_string()];
    let extras: &[&str] = match clean {
        "com.google.chrome" | "chrome" | "google-chrome" | "chromium" => &[
            "chrome",
            "chrome.exe",
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "org.chromium.chromium",
        ],
        "com.microsoft.edgemac" | "msedge" | "edge" | "microsoft-edge" => {
            &["msedge", "msedge.exe", "microsoft-edge", "microsoft-edge-stable"]
        }
        "org.mozilla.firefox" | "firefox" => {
            &["firefox", "firefox.exe", "firefox-esr", "org.mozilla.firefox"]
        }
        "com.microsoft.vscode" | "vscode" | "code" => {
            &["code", "code.exe", "codium", "com.visualstudio.code"]
        }
        "com.apple.calculator" | "calculator" | "calculatorapp" | "calc" => &[
            "calculatorapp",
            "calculatorapp.exe",
            "calc",
            "calc.exe",
            "gnome-calculator",
            "org.gnome.calculator",
            "kcalc",
            "galculator",
        ],
        "com.apple.textedit" | "textedit" | "notepad" => &[
            "notepad",
            "notepad.exe",
            "gedit",
            "org.gnome.gedit",
            "gnome-text-editor",
            "org.gnome.texteditor",
            "kate",
            "mousepad",
            "leafpad",
        ],
        "com.apple.terminal" | "terminal" => &[
            "windowsterminal",
            "windowsterminal.exe",
            "wt",
            "wt.exe",
            "cmd",
            "cmd.exe",
            "powershell",
            "powershell.exe",
            "pwsh",
            "gnome-terminal",
            "gnome-terminal-server",
            "kitty",
            "alacritty",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ],
        _ => &[],
    };
    for &e in extras {
        if !out.iter().any(|x| x == e) {
            out.push(e.to_string());
        }
    }
    if let Some((_, tail)) = clean.rsplit_once('.') {
        if !tail.is_empty() && !out.iter().any(|x| x == tail) {
            out.push(tail.to_string());
        }
    }
    out
}

pub fn app_matches(
    requested: &str,
    proc_name: &str,
    wm_class: Option<&str>,
    window_title: Option<&str>,
    pid: i64,
) -> bool {
    if let Some(pid_str) = requested.strip_prefix("pid:") {
        return pid_str.parse::<i64>().map(|p| p == pid).unwrap_or(false);
    }
    let candidates = alias_candidates(requested);
    let proc_lower = proc_name.trim().to_ascii_lowercase();
    let proc_base = proc_lower
        .strip_suffix(".exe")
        .or_else(|| proc_lower.strip_suffix(".desktop"))
        .unwrap_or(&proc_lower);

    for cand in &candidates {
        if proc_lower == *cand || proc_base == *cand {
            return true;
        }
        if let Some(cls) = wm_class {
            let cls_low = cls.trim().to_ascii_lowercase();
            let cls_tail = cls_low.rsplit('.').next().unwrap_or(&cls_low);
            if cls_low == *cand || cls_tail == *cand {
                return true;
            }
        }
        if let Some(title) = window_title {
            if cand.len() >= 4 && title.to_ascii_lowercase().contains(cand) {
                return true;
            }
        }
    }
    false
}

/// Parses `tesseract ... tsv` output into logical screen-coordinate `OcrBox` spans.
pub fn parse_tesseract_tsv(
    tsv: &str,
    origin_x: f64,
    origin_y: f64,
    scale_x: f64,
    scale_y: f64,
) -> Vec<OcrBox> {
    let sx = if scale_x > 0.0 { scale_x } else { 1.0 };
    let sy = if scale_y > 0.0 { scale_y } else { 1.0 };
    let mut out = Vec::new();
    for (idx, line) in tsv.lines().enumerate() {
        if idx == 0 && line.starts_with("level\t") {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }
        let Ok(conf) = cols[10].trim().parse::<f64>() else {
            continue;
        };
        if conf < 0.0 {
            continue;
        }
        let (Ok(l), Ok(t), Ok(w), Ok(h)) = (
            cols[6].trim().parse::<f64>(),
            cols[7].trim().parse::<f64>(),
            cols[8].trim().parse::<f64>(),
            cols[9].trim().parse::<f64>(),
        ) else {
            continue;
        };
        if w < 2.0 || h < 2.0 {
            continue;
        }
        out.push(OcrBox {
            text: text.to_string(),
            confidence: (conf / 100.0).clamp(0.0, 1.0),
            frame: Rect {
                x: origin_x + l / sx,
                y: origin_y + t / sy,
                w: w / sx,
                h: h / sy,
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_roles_and_redacts_password_elements() {
        let snap = build_snapshot_from_raw(
            "firefox".into(),
            "firefox".into(),
            1234,
            true,
            vec![WindowMeta {
                index: 0,
                title: "Login".into(),
                frame: Some(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 800.0,
                    h: 600.0,
                }),
            }],
            vec![
                RawElement {
                    role: "push button".into(),
                    label: "Sign In".into(),
                    value: None,
                    window: 0,
                    depth: 1,
                    enabled: Some(true),
                    focused: false,
                    is_password: false,
                    offscreen: false,
                    actions: vec![],
                    frame: Some(Rect {
                        x: 50.0,
                        y: 120.0,
                        w: 90.0,
                        h: 32.0,
                    }),
                },
                RawElement {
                    role: "edit".into(),
                    label: "Secret".into(),
                    value: Some("hunter2".into()),
                    window: 0,
                    depth: 1,
                    enabled: Some(true),
                    focused: true,
                    is_password: true,
                    offscreen: false,
                    actions: vec![],
                    frame: Some(Rect {
                        x: 50.0,
                        y: 70.0,
                        w: 180.0,
                        h: 28.0,
                    }),
                },
            ],
            100,
            10,
        );
        assert!(snap.has_secure_field);
        assert_eq!(snap.elements.len(), 2);
        assert_eq!(snap.elements[0].ref_id, "e1");
        assert_eq!(snap.elements[0].role, "Button");
        assert_eq!(snap.elements[1].ref_id, "");
        assert_eq!(snap.elements[1].role, "SecureTextField");
        assert_eq!(snap.elements[1].value, None);
    }

    #[test]
    fn alias_matching_and_tesseract_tsv_parse() {
        assert!(app_matches("com.google.Chrome", "chrome.exe", None, None, 99));
        assert!(app_matches("com.google.Chrome", "google-chrome", None, None, 99));
        assert!(app_matches("pid:99", "anything", None, None, 99));
        assert!(!app_matches("pid:100", "chrome", None, None, 99));

        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   5\t1\t1\t1\t1\t1\t100\t200\t80\t24\t92\tFlights\n\
                   5\t1\t1\t1\t1\t2\t190\t200\t60\t24\t-1\t\n";
        let boxes = parse_tesseract_tsv(tsv, 50.0, 40.0, 2.0, 2.0);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].text, "Flights");
        assert!((boxes[0].frame.x - 100.0).abs() < 0.01);
        assert!((boxes[0].frame.y - 140.0).abs() < 0.01);
        assert!((boxes[0].confidence - 0.92).abs() < 0.01);
    }
}
