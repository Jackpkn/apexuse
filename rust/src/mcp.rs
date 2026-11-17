use crate::gateway::Gateway;
use crate::json::{parse_json, JsonValue};
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

fn type_schema(ty: &str) -> JsonValue {
    JsonValue::object(vec![("type", JsonValue::String(ty.into()))])
}

fn bool_schema(desc: &str) -> JsonValue {
    JsonValue::object(vec![
        ("type", JsonValue::String("boolean".into())),
        ("description", JsonValue::String(desc.into())),
    ])
}

fn target_schema() -> JsonValue {
    JsonValue::object(vec![
        ("type", JsonValue::String("object".into())),
        (
            "properties",
            JsonValue::object(vec![
                ("query", type_schema("string")),
                ("role", type_schema("string")),
                ("fresh", type_schema("boolean")),
            ]),
        ),
    ])
}

fn common_action_argument_props() -> Vec<(&'static str, JsonValue)> {
    vec![
        ("app", type_schema("string")),
        ("window", type_schema("integer")),
        ("ref", type_schema("string")),
        ("mark", type_schema("string")),
        ("target", target_schema()),
        ("x", type_schema("number")),
        ("y", type_schema("number")),
        ("from_ref", type_schema("string")),
        ("from_mark", type_schema("string")),
        ("to_ref", type_schema("string")),
        ("to_mark", type_schema("string")),
        ("x1", type_schema("number")),
        ("y1", type_schema("number")),
        ("x2", type_schema("number")),
        ("y2", type_schema("number")),
        ("include_bounds", include_bounds_schema()),
        ("return_state", return_state_schema()),
        ("verify_mode", verify_mode_schema()),
        ("keep_snapshot", type_schema("boolean")),
        ("verify_change", type_schema("boolean")),
        ("require_change", type_schema("boolean")),
        ("max_retries", type_schema("integer")),
        ("settle_ms", type_schema("integer")),
        ("poll_interval_ms", type_schema("integer")),
        ("retry_delay_ms", type_schema("integer")),
    ]
}

fn computer_argument_properties() -> JsonValue {
    let mut props = common_action_argument_props();
    props.extend([
        ("text", type_schema("string")),
        ("mode", type_schema("string")),
        ("start", type_schema("integer")),
        ("end", type_schema("integer")),
        ("click_first", type_schema("boolean")),
        ("key", type_schema("string")),
        ("modifiers", type_schema("array")),
        ("direction", type_schema("string")),
        ("amount", type_schema("integer")),
        (
            "steps",
            JsonValue::object(vec![
                ("type", JsonValue::String("array".into())),
                (
                    "description",
                    JsonValue::String(
                        "Up to 24 action steps; each step supports verify_mode, verify_change, diff, and include_bounds."
                            .into(),
                    ),
                ),
            ]),
        ),
        ("inspect_after", type_schema("boolean")),
        ("diff", diff_schema()),
        ("query", type_schema("string")),
        ("role", type_schema("string")),
        ("timeout_ms", type_schema("integer")),
        ("poll_ms", type_schema("integer")),
        ("absent", type_schema("boolean")),
        ("ocr", type_schema("boolean")),
        ("token", type_schema("string")),
    ]);
    JsonValue::object(props)
}

fn desktop_argument_properties() -> JsonValue {
    let mut props = common_action_argument_props();
    props.push(("url", type_schema("string")));
    JsonValue::object(props)
}

fn str_enum_schema(values: &[&str], desc: &str) -> JsonValue {
    JsonValue::object(vec![
        ("type", JsonValue::String("string".into())),
        (
            "enum",
            JsonValue::Array(
                values
                    .iter()
                    .map(|v| JsonValue::String((*v).into()))
                    .collect(),
            ),
        ),
        ("description", JsonValue::String(desc.into())),
    ])
}

fn return_state_schema() -> JsonValue {
    str_enum_schema(
        &["none", "diff", "full"],
        "Post-action AX state mode ('none' default, 'diff' returns incremental AX diff and keeps new refs live, 'full' returns full AX tree).",
    )
}

fn verify_mode_schema() -> JsonValue {
    str_enum_schema(
        &["auto", "ax", "visual"],
        "Verification signal when verify_change=true ('auto' AX+visual, 'ax' Accessibility-only skipping screenshots, 'visual' pixel-only).",
    )
}

fn include_bounds_schema() -> JsonValue {
    bool_schema("Include @(x,y,wxh) element coordinates in AX outlines (defaults to config snapshot.include_bounds, false).")
}

fn diff_schema() -> JsonValue {
    bool_schema("Enable incremental AX tree diffing vs prior snapshot of the same app/window (defaults to config snapshot.ax_diff, true).")
}

fn tool_defs() -> Vec<JsonValue> {
    vec![
        JsonValue::object(vec![
            ("name", JsonValue::String("inspect_app".into())),
            ("description", JsonValue::String("AX-first semantic inspection of a permitted app bundle ID or 'desktop' (frontmost app). Auto-diffs repeat views (state_mode='full'|'diff'|'unchanged'|'filtered') to minimize tokens while keeping refs live; pass diff=false for full baseline, include_bounds=true for @(x,y,wxh), or query for filtered lookup. Returns compact accessibility tree with refs (e1, e2...) and learned workflow hints.".into())),
            (
                "annotations",
                JsonValue::object(vec![
                    ("readOnlyHint", JsonValue::Bool(true)),
                    ("destructiveHint", JsonValue::Bool(false)),
                    ("openWorldHint", JsonValue::Bool(false)),
                ]),
            ),
            (
                "inputSchema",
                JsonValue::object(vec![
                    ("type", JsonValue::String("object".into())),
                    (
                        "properties",
                        JsonValue::object(vec![
                            ("app", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("window", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                            ("query", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("diff", diff_schema()),
                            ("include_bounds", include_bounds_schema()),
                        ]),
                    ),
                    ("required", JsonValue::Array(vec![JsonValue::String("app".into())])),
                ]),
            ),
        ]),
        JsonValue::object(vec![
            ("name", JsonValue::String("visual_inspect".into())),
            ("description", JsonValue::String("Set-of-Mark (SoM) visual indexing for an app bundle ID or 'desktop' (full screen): captures window/screen, fuses Accessibility + Vision OCR boxes, overlays [m1]..[mN] badges, and returns annotated image (JPEG default or PNG) + clickable Mark IDs. Pass ocr='auto'|'fast'|'accurate'|'off' (default 'auto'), max_marks (1..=150, default 80), format='jpeg'|'png', quality (1..=100), and max_width (640..=2560).".into())),
            (
                "annotations",
                JsonValue::object(vec![
                    ("readOnlyHint", JsonValue::Bool(true)),
                    ("destructiveHint", JsonValue::Bool(false)),
                    ("openWorldHint", JsonValue::Bool(false)),
                ]),
            ),
            (
                "inputSchema",
                JsonValue::object(vec![
                    ("type", JsonValue::String("object".into())),
                    (
                        "properties",
                        JsonValue::object(vec![
                            ("app", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("window", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                            ("ocr", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("max_marks", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                            (
                                "format",
                                JsonValue::object(vec![
                                    ("type", JsonValue::String("string".into())),
                                    (
                                        "enum",
                                        JsonValue::Array(vec![
                                            JsonValue::String("jpeg".into()),
                                            JsonValue::String("png".into()),
                                        ]),
                                    ),
                                ]),
                            ),
                            ("quality", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                            ("max_width", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                        ]),
                    ),
                    ("required", JsonValue::Array(vec![JsonValue::String("app".into())])),
                ]),
            ),
        ]),
        JsonValue::object(vec![
            ("name", JsonValue::String("computer".into())),
            ("description", JsonValue::String("AX-first semantic & Set-of-Mark interaction: action=inspect (supports diff, include_bounds, window, query), click, double_click, right_click, hover (accepts ref, mark, target: {query, role?, fresh?}, or x/y after unfiltered snapshot), drag (from_ref/from_mark/x1,y1 -> to_ref/to_mark/x2,y2), type (ref, mark, or target), set_value (direct native text assignment/clearing on verified editable ref, mark, or target; text='' clears), select_text (optional start/end offsets on verified editable ref, mark, or target), press_key, scroll (direction, amount, optional target ref/mark/x,y), run_actions (up to 24 chained steps including inline wait_for, inspect, set_value, select_text, target, and per-step verify_mode/diff/include_bounds), wait_for (local AX/OCR poll up to 60s, optional window/role), or confirm (token). Pass return_state='diff'|'full' on actions & run_actions to capture updated AX state inline and keep refreshed refs live without extra inspect_app round-trips; pass verify_mode='ax' with verify_change=true to skip screenshots. Also supports keep_snapshot, max_retries (0..=3), settle_ms, poll_interval_ms, retry_delay_ms, and require_change.".into())),
            (
                "inputSchema",
                JsonValue::object(vec![
                    ("type", JsonValue::String("object".into())),
                    (
                        "properties",
                        JsonValue::object(vec![
                            ("action", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            (
                                "arguments",
                                JsonValue::object(vec![
                                    ("type", JsonValue::String("object".into())),
                                    ("properties", computer_argument_properties()),
                                ]),
                            ),
                        ]),
                    ),
                    (
                        "required",
                        JsonValue::Array(vec![
                            JsonValue::String("action".into()),
                            JsonValue::String("arguments".into()),
                        ]),
                    ),
                ]),
            ),
        ]),
        JsonValue::object(vec![
            ("name", JsonValue::String("desktop".into())),
            ("description", JsonValue::String("Desktop-level operations: action=list_applications, launch_application, activate_application, open_url (instant http/https URL open in permitted browser/app), visual_click, double_click, right_click, hover, or drag (using mark, ref, target, or x/y coordinates). Supports return_state='none'|'diff'|'full' (eliminates extra inspect_app calls after activate/open_url/click), include_bounds, verify_change, verify_mode='auto'|'ax'|'visual' ('ax' skips screenshots), max_retries, settle_ms, poll_interval_ms, retry_delay_ms, and require_change.".into())),
            (
                "inputSchema",
                JsonValue::object(vec![
                    ("type", JsonValue::String("object".into())),
                    (
                        "properties",
                        JsonValue::object(vec![
                            ("action", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            (
                                "arguments",
                                JsonValue::object(vec![
                                    ("type", JsonValue::String("object".into())),
                                    ("properties", desktop_argument_properties()),
                                ]),
                            ),
                        ]),
                    ),
                    (
                        "required",
                        JsonValue::Array(vec![
                            JsonValue::String("action".into()),
                            JsonValue::String("arguments".into()),
                        ]),
                    ),
                ]),
            ),
        ]),
        JsonValue::object(vec![
            ("name", JsonValue::String("screenshot".into())),
            (
                "description",
                JsonValue::String(
                    "Capture an unannotated screenshot of the largest window of a permitted running app after running safety preflight. Supports optional format='jpeg'|'png' (default 'jpeg'), quality (1..=100), and max_width (640..=2560)."
                        .into(),
                ),
            ),
            (
                "annotations",
                JsonValue::object(vec![
                    ("readOnlyHint", JsonValue::Bool(true)),
                    ("destructiveHint", JsonValue::Bool(false)),
                    ("openWorldHint", JsonValue::Bool(false)),
                ]),
            ),
            (
                "inputSchema",
                JsonValue::object(vec![
                    ("type", JsonValue::String("object".into())),
                    (
                        "properties",
                        JsonValue::object(vec![
                            ("app", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            (
                                "format",
                                JsonValue::object(vec![
                                    ("type", JsonValue::String("string".into())),
                                    (
                                        "enum",
                                        JsonValue::Array(vec![
                                            JsonValue::String("jpeg".into()),
                                            JsonValue::String("png".into()),
                                        ]),
                                    ),
                                ]),
                            ),
                            ("quality", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                            ("max_width", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                        ]),
                    ),
                    ("required", JsonValue::Array(vec![JsonValue::String("app".into())])),
                ]),
            ),
        ]),
        JsonValue::object(vec![
            ("name", JsonValue::String("workflow_memory".into())),
            (
                "description",
                JsonValue::String(
                    "Local structural app-workflow memory (never stores passwords, typed text, or set_value text): action=list, lookup, record, replay (executable macro replay with optional step_args), or clear."
                        .into(),
                ),
            ),
            (
                "inputSchema",
                JsonValue::object(vec![
                    ("type", JsonValue::String("object".into())),
                    (
                        "properties",
                        JsonValue::object(vec![
                            ("action", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("app", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("id", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("query", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("goal", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("steps", JsonValue::object(vec![("type", JsonValue::String("array".into()))])),
                            ("step_args", JsonValue::object(vec![("type", JsonValue::String("array".into()))])),
                        ]),
                    ),
                    ("required", JsonValue::Array(vec![JsonValue::String("action".into())])),
                ]),
            ),
        ]),
    ]
}

pub fn handle_rpc_line(gw: &mut Gateway, line: &str) -> Option<String> {
    let req = match parse_json(line) {
        Ok(v) => v,
        Err(e) => {
            let err = JsonValue::object(vec![
                ("jsonrpc", JsonValue::String("2.0".into())),
                ("id", JsonValue::Null),
                (
                    "error",
                    JsonValue::object(vec![
                        ("code", JsonValue::Number(-32700.0)),
                        ("message", JsonValue::String(format!("Parse error: {e}"))),
                    ]),
                ),
            ]);
            return Some(err.to_json_string());
        }
    };

    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(JsonValue::Null);

    // Notifications (no id) must not receive a response.
    let Some(id_val) = id else {
        return None;
    };

    let result = match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2024-11-05");
            Ok(JsonValue::object(vec![
                ("protocolVersion", JsonValue::String(requested.into())),
                (
                    "capabilities",
                    JsonValue::object(vec![(
                        "tools",
                        JsonValue::object(vec![("listChanged", JsonValue::Bool(false))]),
                    )]),
                ),
                (
                    "serverInfo",
                    JsonValue::object(vec![
                        ("name", JsonValue::String("apexuse-mcp".into())),
                        ("version", JsonValue::String(env!("CARGO_PKG_VERSION").into())),
                    ]),
                ),
            ]))
        }
        "ping" => Ok(JsonValue::object(vec![])),
        "tools/list" => Ok(JsonValue::object(vec![(
            "tools",
            JsonValue::Array(tool_defs()),
        )])),
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| JsonValue::object(vec![]));
            let out = gw.dispatch(name, &args);
            let mut content = Vec::new();
            let text = out
                .image_caption
                .clone()
                .unwrap_or_else(|| out.payload.to_json_string());
            content.push(JsonValue::object(vec![
                ("type", JsonValue::String("text".into())),
                ("text", JsonValue::String(text)),
            ]));
            if let Some(img_b64) = out.image_b64 {
                content.push(JsonValue::object(vec![
                    ("type", JsonValue::String("image".into())),
                    ("data", JsonValue::String(img_b64)),
                    ("mimeType", JsonValue::String(out.image_mime_type.into())),
                ]));
            }
            Ok(JsonValue::object(vec![
                ("content", JsonValue::Array(content)),
                ("structuredContent", out.payload),
                ("isError", JsonValue::Bool(false)),
            ]))
        }
        _ => Err((-32601, format!("Method not found: {method}"))),
    };

    let resp = match result {
        Ok(res) => JsonValue::object(vec![
            ("jsonrpc", JsonValue::String("2.0".into())),
            ("id", id_val),
            ("result", res),
        ]),
        Err((code, msg)) => JsonValue::object(vec![
            ("jsonrpc", JsonValue::String("2.0".into())),
            ("id", id_val),
            (
                "error",
                JsonValue::object(vec![
                    ("code", JsonValue::Number(code as f64)),
                    ("message", JsonValue::String(msg)),
                ]),
            ),
        ]),
    };

    Some(resp.to_json_string())
}

pub fn run_stdio_loop(gw: &mut Gateway) -> io::Result<()> {
    let (tx, rx) = mpsc::channel::<io::Result<String>>();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line_res in stdin.lock().lines() {
            let is_err = line_res.is_err();
            if tx.send(line_res).is_err() || is_err {
                break;
            }
        }
    });

    let stdout = io::stdout();
    let mut out = stdout.lock();

    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line_res) => {
                let line = line_res?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Some(resp) = handle_rpc_line(gw, trimmed) {
                    writeln!(out, "{resp}")?;
                    out.flush()?;
                }
                gw.tick();
            }
            Err(RecvTimeoutError::Timeout) => {
                gw.tick();
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::MockDriver;
    use crate::policy::PolicySettings;
    use std::fs;

    #[test]
    fn mcp_protocol_initialize_list_and_visual_call() {
        let dir = std::env::temp_dir().join(format!("apexuse_mcp_unit_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let settings = PolicySettings {
            allowed_apps: vec!["*".into()],
            ..PolicySettings::default()
        };
        let mut gw = Gateway::new(Box::new(MockDriver::new()), settings, dir.clone());

        let init = handle_rpc_line(
            &mut gw,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        )
        .unwrap();
        let init_v = parse_json(&init).unwrap();
        assert_eq!(
            init_v.get("result").and_then(|r| r.get("protocolVersion")).and_then(|v| v.as_str()),
            Some("2025-03-26")
        );
        assert_eq!(
            init_v
                .get("result")
                .and_then(|r| r.get("serverInfo"))
                .and_then(|s| s.get("name"))
                .and_then(|v| v.as_str()),
            Some("apexuse-mcp")
        );
        assert_eq!(
            init_v
                .get("result")
                .and_then(|r| r.get("serverInfo"))
                .and_then(|s| s.get("version"))
                .and_then(|v| v.as_str()),
            Some(env!("CARGO_PKG_VERSION"))
        );

        // Notifications must produce no response
        assert!(handle_rpc_line(&mut gw, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());

        let list = handle_rpc_line(&mut gw, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let list_v = parse_json(&list).unwrap();
        let tools = list_v.get("result").and_then(|r| r.get("tools")).and_then(|v| v.as_array()).unwrap();
        assert_eq!(tools.len(), 6);

        let vis = handle_rpc_line(
            &mut gw,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"visual_inspect","arguments":{"app":"Calculator"}}}"#,
        )
        .unwrap();
        let vis_v = parse_json(&vis).unwrap();
        let vis_res = vis_v.get("result").unwrap();
        let content = vis_res.get("content").and_then(|v| v.as_array()).unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1].get("type").and_then(|v| v.as_str()), Some("image"));
        assert_eq!(content[1].get("mimeType").and_then(|v| v.as_str()), Some("image/jpeg"));
        let vis_sc = vis_res.get("structuredContent").unwrap();
        assert_eq!(vis_sc.get("image_format").and_then(|v| v.as_str()), Some("jpeg"));
        assert!(vis_sc.get("image_bytes").and_then(|v| v.as_i64()).unwrap_or(0) > 0);
        let vis_b64 = content[1].get("data").and_then(|v| v.as_str()).unwrap();
        let vis_bytes = crate::som_png::base64_decode(vis_b64).unwrap();
        assert!(vis_bytes.starts_with(&[0xFF, 0xD8, 0xFF]));

        // Explicit format: "png" on visual_inspect and screenshot
        let vis_png = handle_rpc_line(
            &mut gw,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"visual_inspect","arguments":{"app":"Calculator","format":"png"}}}"#,
        )
        .unwrap();
        let vis_png_v = parse_json(&vis_png).unwrap();
        let vis_png_res = vis_png_v.get("result").unwrap();
        let vis_png_content = vis_png_res.get("content").and_then(|v| v.as_array()).unwrap();
        assert_eq!(vis_png_content[1].get("mimeType").and_then(|v| v.as_str()), Some("image/png"));
        assert_eq!(
            vis_png_res.get("structuredContent").and_then(|s| s.get("image_format")).and_then(|v| v.as_str()),
            Some("png")
        );

        let ss_def = handle_rpc_line(
            &mut gw,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"screenshot","arguments":{"app":"Calculator"}}}"#,
        )
        .unwrap();
        let ss_def_v = parse_json(&ss_def).unwrap();
        let ss_def_res = ss_def_v.get("result").unwrap();
        let ss_def_content = ss_def_res.get("content").and_then(|v| v.as_array()).unwrap();
        assert_eq!(ss_def_content[1].get("mimeType").and_then(|v| v.as_str()), Some("image/jpeg"));
        assert_eq!(
            ss_def_res.get("structuredContent").and_then(|s| s.get("image_format")).and_then(|v| v.as_str()),
            Some("jpeg")
        );

        let ss_png = handle_rpc_line(
            &mut gw,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"screenshot","arguments":{"app":"Calculator","format":"png"}}}"#,
        )
        .unwrap();
        let ss_png_v = parse_json(&ss_png).unwrap();
        let ss_png_res = ss_png_v.get("result").unwrap();
        let ss_png_content = ss_png_res.get("content").and_then(|v| v.as_array()).unwrap();
        assert_eq!(ss_png_content[1].get("mimeType").and_then(|v| v.as_str()), Some("image/png"));
        assert_eq!(
            ss_png_res.get("structuredContent").and_then(|s| s.get("image_format")).and_then(|v| v.as_str()),
            Some("png")
        );

        // Verify tool_defs documents new actions & options
        let list_str = list_v.to_json_string();
        for needle in ["set_value", "select_text", "target", "keep_snapshot", "max_width", "24"] {
            assert!(list_str.contains(needle), "tools/list missing {needle}");
        }

        // Verify Task 9 schema options: diff, include_bounds, return_state, verify_mode
        let by_name = |name: &str| {
            tools
                .iter()
                .find(|t| t.get("name").and_then(|v| v.as_str()) == Some(name))
                .expect("tool missing")
        };
        let inspect_props = by_name("inspect_app")
            .get("inputSchema")
            .and_then(|s| s.get("properties"))
            .unwrap();
        assert!(inspect_props.get("diff").is_some(), "inspect_app missing diff");
        assert!(
            inspect_props.get("include_bounds").is_some(),
            "inspect_app missing include_bounds"
        );

        let computer_props = by_name("computer")
            .get("inputSchema")
            .and_then(|s| s.get("properties"))
            .and_then(|p| p.get("arguments"))
            .and_then(|a| a.get("properties"))
            .unwrap();
        for key in [
            "diff",
            "include_bounds",
            "return_state",
            "verify_mode",
            "from_ref",
            "from_mark",
            "to_ref",
            "to_mark",
            "x1",
            "y1",
            "x2",
            "y2",
            "click_first",
            "target",
        ] {
            assert!(computer_props.get(key).is_some(), "computer missing {key}");
        }
        let computer_target_props = computer_props
            .get("target")
            .and_then(|t| t.get("properties"))
            .unwrap();
        for key in ["query", "role", "fresh"] {
            assert!(computer_target_props.get(key).is_some(), "computer.target missing {key}");
        }

        let desktop_props = by_name("desktop")
            .get("inputSchema")
            .and_then(|s| s.get("properties"))
            .and_then(|p| p.get("arguments"))
            .and_then(|a| a.get("properties"))
            .unwrap();
        for key in [
            "return_state",
            "verify_mode",
            "include_bounds",
            "window",
            "keep_snapshot",
            "from_ref",
            "from_mark",
            "to_ref",
            "to_mark",
            "x1",
            "y1",
            "x2",
            "y2",
            "target",
        ] {
            assert!(desktop_props.get(key).is_some(), "desktop missing {key}");
        }
        let desktop_target_props = desktop_props
            .get("target")
            .and_then(|t| t.get("properties"))
            .unwrap();
        for key in ["query", "role", "fresh"] {
            assert!(desktop_target_props.get(key).is_some(), "desktop.target missing {key}");
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
