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
            ("description", JsonValue::String("Set-of-Mark (SoM) visual indexing for an app bundle ID or 'desktop' (full screen): captures via ScreenCaptureKit, fuses Accessibility + Vision OCR boxes, overlays [m1]..[mN] badges, and returns annotated PNG + clickable Mark IDs. Pass ocr='auto'|'fast'|'accurate'|'off' (default 'auto') and max_marks (1..=150, default 80).".into())),
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
                        ]),
                    ),
                    ("required", JsonValue::Array(vec![JsonValue::String("app".into())])),
                ]),
            ),
        ]),
        JsonValue::object(vec![
            ("name", JsonValue::String("computer".into())),
            ("description", JsonValue::String("Semantic & Set-of-Mark interaction: action=inspect, click, double_click, right_click, hover (accepts ref, mark, or x/y after unfiltered snapshot), drag (from_ref/from_mark/x1,y1 -> to_ref/to_mark/x2,y2), type (ref or mark), press_key, scroll (direction, amount, optional target ref/mark/x,y), run_actions (up to 12 chained steps without intermediate LLM round-trips), wait_for (local AX/OCR poll up to 60s, optional window), or confirm (token). Actions & steps support optional verify_change, max_retries (0..=3), settle_ms, retry_delay_ms, and require_change.".into())),
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
            ("name", JsonValue::String("desktop".into())),
            ("description", JsonValue::String("Desktop-level operations: action=list_applications, launch_application, activate_application, open_url (instant http/https URL open in permitted browser/app), visual_click, double_click, right_click, hover, or drag (using mark, ref, or x/y coordinates; supports optional verify_change, max_retries, settle_ms, retry_delay_ms, require_change).".into())),
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
                    "Capture an unannotated screenshot of the largest window of a permitted running app after running safety preflight."
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
                        JsonValue::object(vec![(
                            "app",
                            JsonValue::object(vec![("type", JsonValue::String("string".into()))]),
                        )]),
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
                    "Local structural app-workflow memory (never stores passwords or typed text): action=list, lookup, record, replay (executable macro replay with optional step_args), or clear."
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
                        ("version", JsonValue::String("0.2.1".into())),
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
            if let Some(img_b64) = out.image_png_b64 {
                content.push(JsonValue::object(vec![
                    ("type", JsonValue::String("image".into())),
                    ("data", JsonValue::String(img_b64)),
                    ("mimeType", JsonValue::String("image/png".into())),
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
        let content = vis_v.get("result").and_then(|r| r.get("content")).and_then(|v| v.as_array()).unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1].get("type").and_then(|v| v.as_str()), Some("image"));
        let _ = fs::remove_dir_all(&dir);
    }
}
