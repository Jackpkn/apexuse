use crate::json::{parse_json, JsonValue};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MAX_STDERR_BYTES: usize = 4096;
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const EXPECTED_SERVER_NAME: &str = "apexuse-mcp";

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn write_rpc_message<W: Write>(writer: &mut W, msg: &JsonValue, stage: &str) -> Result<(), String> {
    writeln!(writer, "{}", msg.to_json_string())
        .map_err(|e| format!("failed to write MCP {stage} message: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("failed to flush MCP {stage} message: {e}"))
}

fn is_notification(msg: &JsonValue) -> bool {
    msg.get("id").is_none()
        && msg.get("method").and_then(|v| v.as_str()).is_some()
        && msg.get("result").is_none()
        && msg.get("error").is_none()
}

fn read_rpc_response(
    rx: &Receiver<Result<Option<String>, String>>,
    expected_id: i64,
    stage: &str,
    deadline: Instant,
    timeout: Duration,
) -> Result<JsonValue, String> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "timed out waiting for MCP {stage} response after {}ms",
                timeout.as_millis()
            ));
        }
        let remaining = deadline.saturating_duration_since(now);
        match rx.recv_timeout(remaining) {
            Ok(Ok(Some(line))) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let msg = parse_json(trimmed)
                    .map_err(|e| format!("malformed JSON from MCP server during {stage}: {e}"))?;
                if msg.as_object().is_none() {
                    return Err(format!(
                        "malformed JSON-RPC response during {stage}: expected JSON object"
                    ));
                }
                if is_notification(&msg) {
                    continue;
                }
                let got_id = msg.get("id").and_then(|v| v.as_i64());
                if got_id != Some(expected_id) {
                    if let Some(err_val) = msg.get("error").filter(|v| !v.is_null()) {
                        return Err(format!(
                            "MCP server returned error during {stage}: {}",
                            err_val.to_json_string()
                        ));
                    }
                    let id_desc = msg
                        .get("id")
                        .map(|v| v.to_json_string())
                        .unwrap_or_else(|| "<missing>".to_string());
                    return Err(format!(
                        "unexpected MCP response id during {stage}: expected {expected_id}, got {id_desc}"
                    ));
                }
                if let Some(err_val) = msg.get("error").filter(|v| !v.is_null()) {
                    return Err(format!(
                        "MCP server returned error during {stage}: {}",
                        err_val.to_json_string()
                    ));
                }
                return Ok(msg);
            }
            Ok(Ok(None)) | Err(RecvTimeoutError::Disconnected) => {
                return Err(format!(
                    "MCP server closed stdout before completing {stage}"
                ));
            }
            Ok(Err(read_err)) => {
                return Err(format!(
                    "failed reading MCP stdout during {stage}: {read_err}"
                ));
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "timed out waiting for MCP {stage} response after {}ms",
                    timeout.as_millis()
                ));
            }
        }
    }
}

pub fn probe_mcp_streams<W: Write>(
    rx: Receiver<Result<Option<String>, String>>,
    mut writer: W,
    timeout: Duration,
) -> Result<JsonValue, String> {
    if timeout.is_zero() {
        return Err("probe timeout must be > 0ms".to_string());
    }
    let deadline = Instant::now() + timeout;

    let init_req = JsonValue::object(vec![
        ("jsonrpc", JsonValue::String("2.0".into())),
        ("id", JsonValue::Number(1.0)),
        ("method", JsonValue::String("initialize".into())),
        (
            "params",
            JsonValue::object(vec![
                (
                    "protocolVersion",
                    JsonValue::String(MCP_PROTOCOL_VERSION.into()),
                ),
                ("capabilities", JsonValue::object(vec![])),
                (
                    "clientInfo",
                    JsonValue::object(vec![
                        ("name", JsonValue::String("apexuse-installer".into())),
                        ("version", JsonValue::String(env!("CARGO_PKG_VERSION").into())),
                    ]),
                ),
            ]),
        ),
    ]);
    write_rpc_message(&mut writer, &init_req, "initialize")?;

    let init_resp = read_rpc_response(&rx, 1, "initialize", deadline, timeout)?;
    let init_result = init_resp
        .get("result")
        .ok_or_else(|| "MCP initialize response missing 'result'".to_string())?;
    let server_info = init_result
        .get("serverInfo")
        .ok_or_else(|| "MCP initialize response missing 'result.serverInfo'".to_string())?;
    let server_name = server_info
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "MCP initialize response missing 'result.serverInfo.name'".to_string())?;
    if server_name != EXPECTED_SERVER_NAME {
        return Err(format!(
            "unexpected MCP serverInfo.name: expected '{EXPECTED_SERVER_NAME}', got '{server_name}'"
        ));
    }
    let server_version = server_info
        .get("version")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            "MCP initialize response missing non-empty 'result.serverInfo.version'".to_string()
        })?
        .to_string();

    let initialized_notif = JsonValue::object(vec![
        ("jsonrpc", JsonValue::String("2.0".into())),
        (
            "method",
            JsonValue::String("notifications/initialized".into()),
        ),
        ("params", JsonValue::object(vec![])),
    ]);
    write_rpc_message(&mut writer, &initialized_notif, "notifications/initialized")?;

    let list_req = JsonValue::object(vec![
        ("jsonrpc", JsonValue::String("2.0".into())),
        ("id", JsonValue::Number(2.0)),
        ("method", JsonValue::String("tools/list".into())),
        ("params", JsonValue::object(vec![])),
    ]);
    write_rpc_message(&mut writer, &list_req, "tools/list")?;

    let list_resp = read_rpc_response(&rx, 2, "tools/list", deadline, timeout)?;
    let tools_arr = list_resp
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .ok_or_else(|| "MCP tools/list response missing 'result.tools' array".to_string())?;
    if tools_arr.is_empty() {
        return Err("MCP tools/list returned empty 'result.tools' array".to_string());
    }

    let mut seen = BTreeSet::new();
    let mut tools = Vec::with_capacity(tools_arr.len());
    for (idx, item) in tools_arr.iter().enumerate() {
        if item.as_object().is_none() {
            return Err(format!(
                "MCP tools/list tool at index {idx} must be a JSON object"
            ));
        }
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                format!("MCP tools/list tool at index {idx} missing non-empty string 'name'")
            })?;
        if !seen.insert(name.to_string()) {
            return Err(format!(
                "MCP tools/list returned duplicate tool name '{name}'"
            ));
        }
        tools.push(name.to_string());
    }

    let tools_count = tools.len();
    Ok(JsonValue::object(vec![
        ("ok", JsonValue::Bool(true)),
        ("server_name", JsonValue::String(server_name.to_string())),
        ("server_version", JsonValue::String(server_version)),
        ("tools_count", JsonValue::Number(tools_count as f64)),
        (
            "tools",
            JsonValue::Array(tools.into_iter().map(JsonValue::String).collect()),
        ),
    ]))
}

pub fn probe_mcp_server(
    cmd: &str,
    args: &[String],
    policy_path: Option<&Path>,
    timeout: Duration,
) -> Result<JsonValue, String> {
    if timeout.is_zero() {
        return Err("probe timeout must be > 0ms".to_string());
    }

    let mut command = Command::new(cmd);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = policy_path {
        command.env("APEXUSE_POLICY", path);
    }

    let child = command
        .spawn()
        .map_err(|e| format!("failed to spawn MCP server '{cmd}': {e}"))?;
    let mut guard = ChildGuard(Some(child));

    let stdin = guard
        .0
        .as_mut()
        .and_then(|c| c.stdin.take())
        .ok_or_else(|| "failed to open child stdin pipe".to_string())?;
    let stdout = guard
        .0
        .as_mut()
        .and_then(|c| c.stdout.take())
        .ok_or_else(|| "failed to open child stdout pipe".to_string())?;
    let mut stderr = guard
        .0
        .as_mut()
        .and_then(|c| c.stderr.take())
        .ok_or_else(|| "failed to open child stderr pipe".to_string())?;

    let captured_stderr = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured_clone = Arc::clone(&captured_stderr);
    let (stderr_done_tx, stderr_done_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut out = captured_clone.lock().unwrap_or_else(|e| e.into_inner());
                    if out.len() < MAX_STDERR_BYTES {
                        let take = (MAX_STDERR_BYTES - out.len()).min(n);
                        out.extend_from_slice(&buf[..take]);
                    }
                }
            }
        }
        let _ = stderr_done_tx.send(());
    });

    let (stdout_tx, stdout_rx) = mpsc::channel::<Result<Option<String>, String>>();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = stdout_tx.send(Ok(None));
                    break;
                }
                Ok(_) => {
                    if stdout_tx.send(Ok(Some(line))).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = stdout_tx.send(Err(format!("failed reading stdout: {e}")));
                    break;
                }
            }
        }
    });

    let res = probe_mcp_streams(stdout_rx, stdin, timeout);
    // Terminate & reap child immediately so stderr closes and never leaks processes.
    drop(guard);

    match res {
        Ok(val) => Ok(val),
        Err(err) => {
            let _ = stderr_done_rx.recv_timeout(Duration::from_millis(100));
            let bytes = captured_stderr
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let snippet = String::from_utf8_lossy(&bytes).trim().to_string();
            if snippet.is_empty() {
                Err(err)
            } else {
                Err(format!("{err} (stderr: {snippet})"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_json;
    use std::sync::mpsc;
    use std::time::Instant;

    #[test]
    fn valid_initialize_and_tools_list_handshake_skips_empty_lines_and_notifications() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some("\n".to_string()))).unwrap();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}"#
                .to_string(),
        )))
        .unwrap();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"apexuse-mcp","version":"0.3.0"}}}"#
                .to_string(),
        )))
        .unwrap();
        tx.send(Ok(Some("   \r\n".to_string()))).unwrap();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#.to_string(),
        )))
        .unwrap();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"inspect_app"},{"name":"visual_inspect"},{"name":"computer"},{"name":"desktop"},{"name":"screenshot"},{"name":"workflow_memory"}]}}"#
                .to_string(),
        )))
        .unwrap();

        let mut sent = Vec::new();
        let res = probe_mcp_streams(rx, &mut sent, Duration::from_secs(2))
            .expect("valid handshake should succeed");

        assert_eq!(res.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            res.get("server_name").and_then(|v| v.as_str()),
            Some("apexuse-mcp")
        );
        assert_eq!(
            res.get("server_version").and_then(|v| v.as_str()),
            Some("0.3.0")
        );
        assert_eq!(res.get("tools_count").and_then(|v| v.as_i64()), Some(6));
        let tools: Vec<&str> = res
            .get("tools")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(
            tools,
            vec![
                "inspect_app",
                "visual_inspect",
                "computer",
                "desktop",
                "screenshot",
                "workflow_memory"
            ]
        );

        let sent_text = String::from_utf8(sent).unwrap();
        let lines: Vec<&str> = sent_text.lines().collect();
        assert_eq!(lines.len(), 3, "expected 3 JSON-RPC lines, got: {sent_text}");

        let msg1 = parse_json(lines[0]).unwrap();
        assert_eq!(msg1.get("id").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(
            msg1.get("method").and_then(|v| v.as_str()),
            Some("initialize")
        );
        assert_eq!(
            msg1.get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str()),
            Some("2024-11-05")
        );
        assert_eq!(
            msg1.get("params")
                .and_then(|p| p.get("clientInfo"))
                .and_then(|c| c.get("name"))
                .and_then(|v| v.as_str()),
            Some("apexuse-installer")
        );

        let msg2 = parse_json(lines[1]).unwrap();
        assert!(msg2.get("id").is_none());
        assert_eq!(
            msg2.get("method").and_then(|v| v.as_str()),
            Some("notifications/initialized")
        );

        let msg3 = parse_json(lines[2]).unwrap();
        assert_eq!(msg3.get("id").and_then(|v| v.as_i64()), Some(2));
        assert_eq!(
            msg3.get("method").and_then(|v| v.as_str()),
            Some("tools/list")
        );
    }

    #[test]
    fn rejects_wrong_server_name_and_rpc_errors_and_bad_ids() {
        // Wrong serverInfo.name
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"other-server","version":"1.0.0"}}}"#
                .to_string(),
        )))
        .unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("wrong server name must fail");
        assert!(err.contains("apexuse-mcp"), "unexpected error: {err}");

        // JSON-RPC error response
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not supported"}}"#
                .to_string(),
        )))
        .unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("error response must fail");
        assert!(err.contains("not supported"), "unexpected error: {err}");

        // JSON-RPC error with id: null must not be skipped
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse fail"}}"#
                .to_string(),
        )))
        .unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("null-id error must fail");
        assert!(err.contains("parse fail"), "unexpected error: {err}");

        // Mismatched id
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":99,"result":{"serverInfo":{"name":"apexuse-mcp","version":"0.3.0"}}}"#
                .to_string(),
        )))
        .unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("mismatched id must fail");
        assert!(err.contains("id"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_empty_duplicate_or_invalid_tools_list() {
        let init_ok = r#"{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"apexuse-mcp","version":"0.3.0"}}}"#;

        // Empty tools array
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some(init_ok.to_string()))).unwrap();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#.to_string(),
        )))
        .unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("empty tools array must fail");
        assert!(err.contains("empty"), "unexpected error: {err}");

        // Blank tool name
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some(init_ok.to_string()))).unwrap();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"   "}]}}"#.to_string(),
        )))
        .unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("blank tool name must fail");
        assert!(err.contains("name"), "unexpected error: {err}");

        // Duplicate tool name
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some(init_ok.to_string()))).unwrap();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"computer"},{"name":"computer"}]}}"#
                .to_string(),
        )))
        .unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("duplicate tool name must fail");
        assert!(err.contains("duplicate"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_malformed_json_premature_eof_and_timeout() {
        // Zero timeout rejected
        let (_tx, rx) = mpsc::channel();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::ZERO)
            .expect_err("zero timeout must fail");
        assert!(err.contains("timeout"), "unexpected error: {err}");

        // Malformed JSON
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some("{not valid json".to_string()))).unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("malformed JSON must fail");
        assert!(err.contains("malformed JSON"), "unexpected error: {err}");

        // Premature EOF before initialize
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(None)).unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("premature EOF must fail");
        assert!(err.contains("closed stdout"), "unexpected error: {err}");

        // Premature EOF before tools/list
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Some(
            r#"{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"apexuse-mcp","version":"0.3.0"}}}"#
                .to_string(),
        )))
        .unwrap();
        tx.send(Ok(None)).unwrap();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_secs(1))
            .expect_err("premature EOF during tools/list must fail");
        assert!(err.contains("tools/list"), "unexpected error: {err}");

        // Timeout expiry
        let (_tx_keep_alive, rx) = mpsc::channel();
        let err = probe_mcp_streams(rx, Vec::new(), Duration::from_millis(40))
            .expect_err("timeout must fail");
        assert!(err.contains("timed out"), "unexpected error: {err}");
    }

    #[test]
    fn real_command_spawn_failure_eof_and_timeout_reaping() {
        // Invalid command fails cleanly
        let err = probe_mcp_server(
            "apexuse-nonexistent-binary-xyz-99999",
            &[],
            None,
            Duration::from_millis(200),
        )
        .expect_err("missing executable must fail");
        assert!(err.contains("failed to spawn"), "unexpected error: {err}");

        // Cross-platform real Command EOF / non-MCP output using current test runner --help
        let self_exe = std::env::current_exe().unwrap();
        let self_cmd = self_exe.to_string_lossy().into_owned();
        let err = probe_mcp_server(
            &self_cmd,
            &["--help".to_string()],
            None,
            Duration::from_secs(2),
        )
        .expect_err("non-MCP command must fail");
        assert!(
            err.contains("malformed JSON")
                || err.contains("closed stdout")
                || err.contains("failed to write"),
            "unexpected error: {err}"
        );

        #[cfg(unix)]
        {
            // Surfaces bounded stderr snippet on startup crash / EOF and passes APEXUSE_POLICY
            let err = probe_mcp_server(
                "sh",
                &[
                    "-c".into(),
                    "printf 'startup_boom:%s\n' \"$APEXUSE_POLICY\" >&2; exit 1".into(),
                ],
                Some(Path::new("/tmp/custom-probe-policy.yaml")),
                Duration::from_secs(2),
            )
            .expect_err("crashing server must fail");
            assert!(
                err.contains("startup_boom:/tmp/custom-probe-policy.yaml"),
                "missing stderr/policy snippet: {err}"
            );

            // Chatty stderr > 4096 bytes does not deadlock and is bounded
            let err = probe_mcp_server(
                "sh",
                &[
                    "-c".into(),
                    "head -c 12000 /dev/zero | tr '\\0' 'x' >&2; exit 1".into(),
                ],
                None,
                Duration::from_secs(2),
            )
            .expect_err("chatty stderr exit must fail");
            assert!(
                err.len() <= 4500,
                "stderr snippet should be bounded around 4096 bytes, got len {}",
                err.len()
            );

            // Hung process times out promptly and is killed/reaped
            let start = Instant::now();
            let err = probe_mcp_server(
                "sh",
                &["-c".into(), "sleep 5".into()],
                None,
                Duration::from_millis(80),
            )
            .expect_err("hung process must time out");
            assert!(err.contains("timed out"), "unexpected error: {err}");
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "timeout + reaping took too long: {:?}",
                start.elapsed()
            );
        }
    }
}
