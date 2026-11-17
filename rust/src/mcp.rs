use crate::gateway::Gateway;
use crate::json::{parse_json, JsonValue};
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

fn tool_defs() -> Vec<JsonValue> {
    vec![
        JsonValue::object(vec![
            ("name", JsonValue::String("inspect_app".into())),
            ("description", JsonValue::String("Read-only semantic inspection of a permitted app bundle ID or 'desktop' (frontmost app). Returns compact accessibility tree with refs (e1, e2...), bounding boxes, and learned workflow hints.".into())),
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
            ("description", JsonValue::String("Semantic & Set-of-Mark interaction: action=inspect, click, double_click, right_click, hover (accepts ref, mark, target: {query, role?, fresh?}, or x/y after unfiltered snapshot), drag (from_ref/from_mark/x1,y1 -> to_ref/to_mark/x2,y2), type (ref, mark, or target), set_value (direct native text assignment/clearing on verified editable ref, mark, or target; text='' clears), select_text (optional start/end offsets on verified editable ref, mark, or target), press_key, scroll (direction, amount, optional target ref/mark/x,y), run_actions (up to 24 chained steps including inline wait_for, inspect, set_value, select_text, and target), wait_for (local AX/OCR poll up to 60s, optional window/role), or confirm (token). Actions & steps support optional keep_snapshot, verify_change, max_retries (0..=3), settle_ms, poll_interval_ms, retry_delay_ms, and require_change.".into())),
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
                                    (
                                        "properties",
                                        JsonValue::object(vec![
                                            ("app", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("window", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("ref", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("mark", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            (
                                                "target",
                                                JsonValue::object(vec![
                                                    ("type", JsonValue::String("object".into())),
                                                    (
                                                        "properties",
                                                        JsonValue::object(vec![
                                                            ("query", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                                            ("role", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                                            ("fresh", JsonValue::object(vec![("type", JsonValue::String("boolean".into()))])),
                                                        ]),
                                                    ),
                                                ]),
                                            ),
                                            ("text", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("mode", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("start", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("end", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("key", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("modifiers", JsonValue::object(vec![("type", JsonValue::String("array".into()))])),
                                            ("direction", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("amount", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("steps", JsonValue::object(vec![("type", JsonValue::String("array".into()))])),
                                            ("inspect_after", JsonValue::object(vec![("type", JsonValue::String("boolean".into()))])),
                                            ("keep_snapshot", JsonValue::object(vec![("type", JsonValue::String("boolean".into()))])),
                                            ("verify_change", JsonValue::object(vec![("type", JsonValue::String("boolean".into()))])),
                                            ("require_change", JsonValue::object(vec![("type", JsonValue::String("boolean".into()))])),
                                            ("max_retries", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("settle_ms", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("poll_interval_ms", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("retry_delay_ms", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("query", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("role", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("timeout_ms", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("poll_ms", JsonValue::object(vec![("type", JsonValue::String("integer".into()))])),
                                            ("absent", JsonValue::object(vec![("type", JsonValue::String("boolean".into()))])),
                                            ("ocr", JsonValue::object(vec![("type", JsonValue::String("boolean".into()))])),
                                            ("token", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                                            ("x", JsonValue::object(vec![("type", JsonValue::String("number".into()))])),
                                            ("y", JsonValue::object(vec![("type", JsonValue::String("number".into()))])),
                                        ]),
                                    ),
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
            ("description", JsonValue::String("Desktop-level operations: action=list_applications, launch_application, activate_application, open_url (instant http/https URL open in permitted browser/app), visual_click, double_click, right_click, hover, or drag (using mark, ref, target, or x/y coordinates; supports optional verify_change, max_retries, settle_ms, poll_interval_ms, retry_delay_ms, require_change).".into())),
            (
                "inputSchema",
                JsonValue::object(vec![
                    ("type", JsonValue::String("object".into())),
                    (
                        "properties",
                        JsonValue::object(vec![
                            ("action", JsonValue::object(vec![("type", JsonValue::String("string".into()))])),
                            ("arguments", JsonValue::object(vec![("type", JsonValue::String("object".into()))])),
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
                        ("version", JsonValue::String("0.3.0".into())),
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
        let _ = fs::remove_dir_all(&dir);
    }
}
