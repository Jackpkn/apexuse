use crate::json::{parse_json, JsonValue};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const SUPPORTED_AGENTS: &[&str] = &[
    "antigravity",
    "gemini",
    "claude",
    "claude-desktop",
    "codex",
    "cursor",
    "windsurf",
    "vscode",
    "opencode",
];

pub const CODEX_PLUGIN_TARGET: &str = "codex-plugin";
pub const SKILL_MD: &str = include_str!("../assets/SKILL.md");
pub const CODEX_MARKETPLACE_JSON: &str = include_str!("../assets/plugin/marketplace.json");
pub const CODEX_PLUGIN_JSON: &str = include_str!("../assets/plugin/plugin.json");
pub const CODEX_PLUGIN_SVG: &str = include_str!("../assets/plugin/apexuse.svg");
pub const CODEX_PLUGIN_SMALL_SVG: &str = include_str!("../assets/plugin/apexuse-small.svg");

static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallScope {
    User,
    Project,
}

impl InstallScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallScope::User => "user",
            InstallScope::Project => "project",
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub write: bool,
    pub dry_run: bool,
    pub scope: InstallScope,
    pub use_npx: bool,
    pub policy_path: Option<PathBuf>,
    pub with_skill: bool,
    pub probe: bool,
    pub probe_timeout_ms: u64,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            write: false,
            dry_run: false,
            scope: InstallScope::User,
            use_npx: false,
            policy_path: None,
            with_skill: false,
            probe: false,
            probe_timeout_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetOs {
    Macos,
    Windows,
    Linux,
}

impl TargetOs {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            TargetOs::Macos
        } else if cfg!(target_os = "windows") {
            TargetOs::Windows
        } else {
            TargetOs::Linux
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InstallEnv {
    pub home: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub project_dir: PathBuf,
    pub path_dirs: Vec<PathBuf>,
    pub applications_dir: Option<PathBuf>,
    pub os: TargetOs,
}

impl InstallEnv {
    pub(crate) fn from_system() -> Self {
        let home = resolve_home_dir();
        let app_data = env::var("APPDATA")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        let xdg_config = env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        let project_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let path_dirs = env::var_os("PATH")
            .map(|p| env::split_paths(&p).collect())
            .unwrap_or_default();
        let os = TargetOs::current();
        let applications_dir = if os == TargetOs::Macos {
            Some(PathBuf::from("/Applications"))
        } else {
            None
        };
        Self {
            home,
            app_data,
            xdg_config,
            project_dir,
            path_dirs,
            applications_dir,
            os,
        }
    }
}

fn resolve_home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let keys = ["USERPROFILE", "HOME"];
    #[cfg(not(target_os = "windows"))]
    let keys = ["HOME", "USERPROFILE"];

    for key in keys {
        if let Ok(val) = env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(PathBuf::from(trimmed));
            }
        }
    }
    if let (Ok(drive), Ok(path)) = (env::var("HOMEDRIVE"), env::var("HOMEPATH")) {
        let joined = format!("{drive}{path}");
        if !joined.trim().is_empty() {
            return Some(PathBuf::from(joined));
        }
    }
    None
}

pub fn install_agent(
    agent: &str,
    exe_path: &Path,
    opts: &InstallOptions,
) -> Result<JsonValue, String> {
    let env = InstallEnv::from_system();
    install_agent_with_env(agent, exe_path, opts, &env)
}

pub fn install_all(exe_path: &Path, opts: &InstallOptions) -> Result<JsonValue, String> {
    let env = InstallEnv::from_system();
    install_all_with_env(exe_path, opts, &env)
}

pub fn format_json_pretty(val: &JsonValue) -> String {
    let mut out = String::new();
    write_pretty_json(val, 0, &mut out);
    out
}

fn write_pretty_json(val: &JsonValue, indent: usize, out: &mut String) {
    match val {
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
            out.push_str(&val.to_json_string());
        }
        JsonValue::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                push_indent(out, indent + 2);
                write_pretty_json(item, indent + 2, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push(']');
        }
        JsonValue::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let len = map.len();
            for (i, (k, v)) in map.iter().enumerate() {
                push_indent(out, indent + 2);
                out.push_str(&JsonValue::String(k.clone()).to_json_string());
                out.push_str(": ");
                write_pretty_json(v, indent + 2, out);
                if i + 1 < len {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push('}');
        }
    }
}

fn push_indent(out: &mut String, spaces: usize) {
    for _ in 0..spaces {
        out.push(' ');
    }
}

fn require_home(env: &InstallEnv) -> Result<&Path, String> {
    env.home
        .as_deref()
        .ok_or_else(|| "cannot resolve user home directory (HOME / USERPROFILE not set)".into())
}

fn resolve_app_data_dir(env: &InstallEnv) -> Result<PathBuf, String> {
    if let Some(dir) = &env.app_data {
        return Ok(dir.clone());
    }
    Ok(require_home(env)?.join("AppData").join("Roaming"))
}

fn resolve_xdg_config_dir(env: &InstallEnv) -> Result<PathBuf, String> {
    if let Some(dir) = &env.xdg_config {
        return Ok(dir.clone());
    }
    Ok(require_home(env)?.join(".config"))
}

fn resolve_target_path(
    agent: &str,
    scope: InstallScope,
    env: &InstallEnv,
) -> Result<PathBuf, String> {
    match (agent, scope) {
        ("antigravity", InstallScope::User) => {
            Ok(require_home(env)?.join(".gemini").join("antigravity").join("mcp_config.json"))
        }
        ("antigravity", InstallScope::Project) => {
            Ok(env.project_dir.join(".agents").join("mcp_config.json"))
        }
        ("gemini", InstallScope::User) => Ok(require_home(env)?.join(".gemini").join("settings.json")),
        ("gemini", InstallScope::Project) => Ok(env.project_dir.join(".gemini").join("settings.json")),
        ("claude", InstallScope::User) => Ok(require_home(env)?.join(".claude.json")),
        ("claude", InstallScope::Project) => Ok(env.project_dir.join(".mcp.json")),
        ("claude-desktop", InstallScope::User) => {
            let base = match env.os {
                TargetOs::Macos => require_home(env)?
                    .join("Library")
                    .join("Application Support"),
                TargetOs::Windows => resolve_app_data_dir(env)?,
                TargetOs::Linux => resolve_xdg_config_dir(env)?,
            };
            Ok(base.join("Claude").join("claude_desktop_config.json"))
        }
        ("claude-desktop", InstallScope::Project) => Err(
            "claude-desktop only supports --scope user (Claude Desktop has no project-scoped config file)"
                .into(),
        ),
        ("codex", InstallScope::User) => Ok(require_home(env)?.join(".codex").join("config.toml")),
        ("codex", InstallScope::Project) => Ok(env.project_dir.join(".codex").join("config.toml")),
        (CODEX_PLUGIN_TARGET, InstallScope::User) => {
            Ok(require_home(env)?.join(".codex").join("config.toml"))
        }
        (CODEX_PLUGIN_TARGET, InstallScope::Project) => {
            Err("codex-plugin only supports --scope user".into())
        }
        ("cursor", InstallScope::User) => Ok(require_home(env)?.join(".cursor").join("mcp.json")),
        ("cursor", InstallScope::Project) => Ok(env.project_dir.join(".cursor").join("mcp.json")),
        ("windsurf", InstallScope::User) => {
            Ok(require_home(env)?.join(".codeium").join("windsurf").join("mcp_config.json"))
        }
        ("windsurf", InstallScope::Project) => {
            Ok(env.project_dir.join(".codeium").join("windsurf").join("mcp_config.json"))
        }
        ("vscode", InstallScope::User) => {
            let base = match env.os {
                TargetOs::Macos => require_home(env)?
                    .join("Library")
                    .join("Application Support"),
                TargetOs::Windows => resolve_app_data_dir(env)?,
                TargetOs::Linux => resolve_xdg_config_dir(env)?,
            };
            Ok(base.join("Code").join("User").join("mcp.json"))
        }
        ("vscode", InstallScope::Project) => Ok(env.project_dir.join(".vscode").join("mcp.json")),
        ("opencode", InstallScope::User) => {
            Ok(resolve_xdg_config_dir(env)?.join("opencode").join("opencode.json"))
        }
        ("opencode", InstallScope::Project) => Ok(env.project_dir.join("opencode.json")),
        _ => Err(format!(
            "Unsupported agent '{agent}'. Supported: {}, {}, all",
            SUPPORTED_AGENTS.join(", "),
            CODEX_PLUGIN_TARGET
        )),
    }
}

pub fn resolved_install_invocation(
    raw_target: &str,
    exe_path: &Path,
    opts: &InstallOptions,
) -> (String, Vec<String>) {
    let _target = raw_target.trim();
    if opts.use_npx {
        (
            "npx".to_string(),
            vec!["-y".to_string(), "apexuse-mcp".to_string()],
        )
    } else {
        (exe_path.to_string_lossy().into_owned(), Vec::new())
    }
}

fn build_env_object(opts: &InstallOptions) -> Option<JsonValue> {
    opts.policy_path.as_ref().map(|p| {
        JsonValue::object(vec![(
            "APEXUSE_POLICY",
            JsonValue::String(p.to_string_lossy().into_owned()),
        )])
    })
}

fn build_server_entry(agent: &str, cmd: &str, args: &[String], opts: &InstallOptions) -> JsonValue {
    let args_json = JsonValue::Array(args.iter().cloned().map(JsonValue::String).collect());
    let env_obj = build_env_object(opts);

    match agent {
        "vscode" => {
            let mut fields = vec![
                ("type", JsonValue::String("stdio".into())),
                ("command", JsonValue::String(cmd.to_string())),
                ("args", args_json),
            ];
            if let Some(env_val) = env_obj {
                fields.push(("env", env_val));
            }
            JsonValue::object(fields)
        }
        "opencode" => {
            let mut full_cmd = Vec::with_capacity(1 + args.len());
            full_cmd.push(JsonValue::String(cmd.to_string()));
            for a in args {
                full_cmd.push(JsonValue::String(a.clone()));
            }
            let mut fields = vec![
                ("type", JsonValue::String("local".into())),
                ("command", JsonValue::Array(full_cmd)),
                ("enabled", JsonValue::Bool(true)),
            ];
            if let Some(env_val) = env_obj {
                fields.push(("environment", env_val));
            }
            JsonValue::object(fields)
        }
        _ => {
            let mut fields = vec![
                ("command", JsonValue::String(cmd.to_string())),
                ("args", args_json),
            ];
            if let Some(env_val) = env_obj {
                fields.push(("env", env_val));
            }
            JsonValue::object(fields)
        }
    }
}

fn json_container_key(agent: &str) -> &'static str {
    match agent {
        "vscode" => "servers",
        "opencode" => "mcp",
        _ => "mcpServers",
    }
}

fn shell_quote_arg(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'=' | b':'))
    {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn build_claude_cli_hint(cmd: &str, args: &[String], opts: &InstallOptions) -> String {
    let mut parts = vec!["claude".into(), "mcp".into(), "add".into()];
    if opts.scope == InstallScope::Project {
        parts.push("--scope".into());
        parts.push("project".into());
    }
    if let Some(policy) = &opts.policy_path {
        parts.push("--env".into());
        parts.push(shell_quote_arg(&format!(
            "APEXUSE_POLICY={}",
            policy.to_string_lossy()
        )));
    }
    parts.push("apexuse".into());
    parts.push("--".into());
    parts.push(shell_quote_arg(cmd));
    for a in args {
        parts.push(shell_quote_arg(a));
    }
    parts.join(" ")
}

fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn render_codex_toml_block(cmd: &str, args: &[String], opts: &InstallOptions) -> String {
    let args_str = args
        .iter()
        .map(|a| toml_basic_string(a))
        .collect::<Vec<_>>()
        .join(", ");
    let mut block = format!(
        "[mcp_servers.apexuse]\ncommand = {}\nargs = [{}]\nstartup_timeout_sec = 60\ntool_timeout_sec = 180",
        toml_basic_string(cmd),
        args_str
    );
    if let Some(policy) = &opts.policy_path {
        block.push_str(&format!(
            "\n\n[mcp_servers.apexuse.env]\nAPEXUSE_POLICY = {}",
            toml_basic_string(&policy.to_string_lossy())
        ));
    }
    block
}

fn render_codex_marketplace_toml_block(marketplace_path: &Path) -> String {
    format!(
        "[marketplaces.apexuse-local]\nsource_type = \"local\"\nsource = {}",
        toml_basic_string(&marketplace_path.to_string_lossy())
    )
}

fn render_codex_plugin_toml_block() -> &'static str {
    "[plugins.\"apexuse@apexuse-local\"]\nenabled = true"
}

fn render_codex_plugin_mcp_json(cmd: &str, args: &[String], opts: &InstallOptions) -> String {
    let entry = build_server_entry(CODEX_PLUGIN_TARGET, cmd, args, opts);
    let root = JsonValue::object(vec![(
        "mcpServers",
        JsonValue::object(vec![("apexuse", entry)]),
    )]);
    format!("{}\n", format_json_pretty(&root))
}

fn update_toml_multiline_state(line: &str, state: &mut Option<&'static str>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_single_basic: Option<u8> = None;

    while i < bytes.len() {
        if let Some(delim) = *state {
            if delim == "\"\"\"" {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i..].starts_with(b"\"\"\"") {
                    *state = None;
                    i += 3;
                    continue;
                }
            } else if bytes[i..].starts_with(b"'''") {
                *state = None;
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }

        if let Some(q) = in_single_basic {
            if q == b'"' && bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                in_single_basic = None;
            }
            i += 1;
            continue;
        }

        if bytes[i] == b'#' {
            break;
        }
        if bytes[i..].starts_with(b"\"\"\"") {
            *state = Some("\"\"\"");
            i += 3;
            continue;
        }
        if bytes[i..].starts_with(b"'''") {
            *state = Some("'''");
            i += 3;
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_single_basic = Some(bytes[i]);
        }
        i += 1;
    }
}

fn parse_toml_header_segments(line: &str, path: &Path) -> Result<Option<Vec<String>>, String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return Ok(None);
    }
    let is_array = trimmed.starts_with("[[");
    let start_idx = if is_array { 2 } else { 1 };
    let bytes = trimmed.as_bytes();
    let mut i = start_idx;
    let mut quote: Option<u8> = None;
    let mut close_idx: Option<usize> = None;

    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if q == b'"' && b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' {
            quote = Some(b);
            i += 1;
            continue;
        }
        if b == b']' {
            if is_array {
                if i + 1 < bytes.len() && bytes[i + 1] == b']' {
                    close_idx = Some(i);
                    break;
                }
            } else {
                close_idx = Some(i);
                break;
            }
        }
        i += 1;
    }

    let Some(end_idx) = close_idx else {
        return Err(format!(
            "refusing to modify {}: invalid TOML table header '{}'",
            path.display(),
            trimmed
        ));
    };
    let after = trimmed[end_idx + if is_array { 2 } else { 1 }..].trim();
    if !after.is_empty() && !after.starts_with('#') {
        return Err(format!(
            "refusing to modify {}: unexpected trailing content after TOML header '{}'",
            path.display(),
            trimmed
        ));
    }

    let inner = &trimmed[start_idx..end_idx];
    let segments = split_toml_dotted_keys(inner).map_err(|e| {
        format!(
            "refusing to modify {}: invalid TOML table path '{}' ({e})",
            path.display(),
            trimmed
        )
    })?;
    Ok(Some(segments))
}

fn split_toml_dotted_keys(inner: &str) -> Result<Vec<String>, String> {
    let mut segments = Vec::new();
    let mut cur = String::new();
    let mut chars = inner.chars().peekable();
    let mut quote: Option<char> = None;
    let mut had_quoted = false;

    while let Some(ch) = chars.next() {
        if let Some(q) = quote {
            if q == '"' && ch == '\\' {
                if let Some(esc) = chars.next() {
                    cur.push(esc);
                } else {
                    return Err("unterminated escape in table key".into());
                }
            } else if ch == q {
                quote = None;
            } else {
                cur.push(ch);
            }
            continue;
        }
        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                had_quoted = true;
            }
            '.' => {
                let seg = cur.trim().to_string();
                if seg.is_empty() && !had_quoted {
                    return Err("empty table segment".into());
                }
                segments.push(seg);
                cur.clear();
                had_quoted = false;
            }
            _ => cur.push(ch),
        }
    }
    if quote.is_some() {
        return Err("unterminated quote in table header".into());
    }
    let last = cur.trim().to_string();
    if last.is_empty() && !had_quoted {
        return Err("empty table segment".into());
    }
    segments.push(last);
    Ok(segments)
}

fn is_apexuse_codex_table(table: &[String]) -> bool {
    (table.len() == 2 && table[0] == "mcp_servers" && table[1] == "apexuse")
        || (table.len() == 3
            && table[0] == "mcp_servers"
            && table[1] == "apexuse"
            && table[2] == "env")
}

fn is_codex_marketplace_table(table: &[String]) -> bool {
    table.len() == 2 && table[0] == "marketplaces" && table[1] == "apexuse-local"
}

fn is_codex_plugin_table(table: &[String]) -> bool {
    table.len() == 2 && table[0] == "plugins" && table[1] == "apexuse@apexuse-local"
}

struct TomlSection {
    table: Option<Vec<String>>,
    lines: Vec<String>,
}

enum TomlSectionOp<'a> {
    Upsert {
        matcher: fn(&[String]) -> bool,
        block: &'a str,
    },
    Remove {
        matcher: fn(&[String]) -> bool,
    },
    Transform {
        matcher: fn(&[String]) -> bool,
        transform: fn(&mut Vec<String>),
    },
}

fn trim_blank_edges(lines: Vec<String>) -> Vec<String> {
    let start = lines
        .iter()
        .position(|l| !l.trim().is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(start);
    lines[start..end].to_vec()
}

fn parse_toml_sections(raw: &str, path: &Path) -> Result<Vec<TomlSection>, String> {
    let mut sections = vec![TomlSection {
        table: None,
        lines: Vec::new(),
    }];
    let mut multiline: Option<&'static str> = None;

    for raw_line in raw.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line).to_string();
        if multiline.is_none() {
            if let Some(segs) = parse_toml_header_segments(&line, path)? {
                sections.push(TomlSection {
                    table: Some(segs),
                    lines: vec![line],
                });
                continue;
            }
        }
        update_toml_multiline_state(&line, &mut multiline);
        sections.last_mut().unwrap().lines.push(line);
    }

    if multiline.is_some() {
        return Err(format!(
            "refusing to modify {}: unterminated multiline string in TOML",
            path.display()
        ));
    }

    Ok(sections
        .into_iter()
        .filter_map(|sec| {
            let lines = trim_blank_edges(sec.lines);
            if lines.is_empty() {
                None
            } else {
                Some(TomlSection {
                    table: sec.table,
                    lines,
                })
            }
        })
        .collect())
}

fn parse_toml_assignment(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('[') {
        return None;
    }
    let (lhs, rhs) = trimmed.split_once('=')?;
    let k = lhs.trim();
    let k = k
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| k.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(k);
    Some((k, rhs))
}

fn strip_toml_inline_comment(rhs: &str) -> &str {
    let mut quote: Option<u8> = None;
    let bytes = rhs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if q == b'"' && b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                quote = None;
            }
        } else if b == b'"' || b == b'\'' {
            quote = Some(b);
        } else if b == b'#' {
            return rhs[..i].trim();
        }
        i += 1;
    }
    rhs.trim()
}

fn read_toml_section_bool(lines: &[String], key: &str) -> Option<bool> {
    let mut multiline: Option<&'static str> = None;
    let mut found = None;

    for (idx, line) in lines.iter().enumerate() {
        if idx == 0 && line.trim().starts_with('[') {
            continue;
        }
        if multiline.is_none() {
            if let Some((k, rhs)) = parse_toml_assignment(line) {
                if k == key {
                    found = match strip_toml_inline_comment(rhs) {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    };
                }
            }
        }
        update_toml_multiline_state(line, &mut multiline);
    }

    found
}

fn set_toml_section_bool(lines: &mut Vec<String>, key: &str, value: bool) {
    let replacement = format!("{key} = {value}");
    let mut multiline: Option<&'static str> = None;
    let mut replaced = false;

    for (idx, line) in lines.iter_mut().enumerate() {
        if idx == 0 && line.trim().starts_with('[') {
            continue;
        }
        if multiline.is_none() {
            if let Some((k, _)) = parse_toml_assignment(line) {
                if k == key {
                    *line = replacement.clone();
                    replaced = true;
                    continue;
                }
            }
        }
        update_toml_multiline_state(line, &mut multiline);
    }

    if !replaced {
        lines.push(replacement);
    }
}

fn disable_codex_plugin_section(lines: &mut Vec<String>) {
    set_toml_section_bool(lines, "enabled", false);
}

fn apply_toml_section_ops(
    existing: Option<&str>,
    ops: &[TomlSectionOp<'_>],
    path: &Path,
) -> Result<String, String> {
    let mut sections = match existing {
        Some(raw) if !raw.trim().is_empty() => parse_toml_sections(raw, path)?,
        _ => Vec::new(),
    };

    for op in ops {
        match op {
            TomlSectionOp::Remove { matcher } => {
                sections.retain(|s| !s.table.as_deref().is_some_and(matcher));
            }
            TomlSectionOp::Upsert { matcher, block } => {
                let mut retained = Vec::with_capacity(sections.len() + 1);
                let mut insert_at = None;
                for sec in sections {
                    if sec.table.as_deref().is_some_and(matcher) {
                        if insert_at.is_none() {
                            insert_at = Some(retained.len());
                        }
                        continue;
                    }
                    retained.push(sec);
                }
                let pos = insert_at.unwrap_or(retained.len()).min(retained.len());
                retained.insert(
                    pos,
                    TomlSection {
                        table: None,
                        lines: block.lines().map(str::to_string).collect(),
                    },
                );
                sections = retained;
            }
            TomlSectionOp::Transform { matcher, transform } => {
                for sec in &mut sections {
                    if sec.table.as_deref().is_some_and(matcher) {
                        transform(&mut sec.lines);
                    }
                }
            }
        }
    }

    let rendered: Vec<String> = sections
        .into_iter()
        .filter_map(|s| {
            let trimmed = trim_blank_edges(s.lines);
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.join("\n"))
            }
        })
        .collect();
    if rendered.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("{}\n", rendered.join("\n\n")))
    }
}

fn upsert_codex_toml(
    existing: Option<&str>,
    block: &str,
    disable_plugin: bool,
    path: &Path,
) -> Result<String, String> {
    let mut ops = vec![TomlSectionOp::Upsert {
        matcher: is_apexuse_codex_table,
        block,
    }];
    if disable_plugin {
        ops.push(TomlSectionOp::Transform {
            matcher: is_codex_plugin_table,
            transform: disable_codex_plugin_section,
        });
    }
    apply_toml_section_ops(existing, &ops, path)
}

fn upsert_codex_plugin_toml(
    existing: Option<&str>,
    marketplace_block: &str,
    plugin_block: &str,
    path: &Path,
) -> Result<String, String> {
    let ops = [
        TomlSectionOp::Remove {
            matcher: is_apexuse_codex_table,
        },
        TomlSectionOp::Upsert {
            matcher: is_codex_marketplace_table,
            block: marketplace_block,
        },
        TomlSectionOp::Upsert {
            matcher: is_codex_plugin_table,
            block: plugin_block,
        },
    ];
    apply_toml_section_ops(existing, &ops, path)
}

fn append_bak_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".bak");
    PathBuf::from(os)
}

fn atomic_write_bytes(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("failed to create directory {}: {e}", parent.display())
            })?;
        }
    }
    let file_name = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("config");
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(".{file_name}.tmp.{}.{seq}", std::process::id());
    let tmp_path = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(tmp_name),
        _ => PathBuf::from(tmp_name),
    };
    if let Err(e) = fs::write(&tmp_path, data) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!(
            "failed to write temp file {}: {e}",
            tmp_path.display()
        ));
    }
    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!(
            "failed to rename {} to {}: {e}",
            tmp_path.display(),
            path.display()
        ));
    }
    Ok(())
}

fn write_config_safely(
    target_path: &Path,
    existing_raw: Option<&str>,
    new_content: &str,
) -> Result<(&'static str, Option<PathBuf>), String> {
    if existing_raw == Some(new_content) {
        return Ok(("unchanged", None));
    }
    let mut backup_path = None;
    if let Some(old) = existing_raw {
        if !old.trim().is_empty() {
            let bak = append_bak_path(target_path);
            atomic_write_bytes(&bak, old.as_bytes())?;
            backup_path = Some(bak);
        }
    }
    atomic_write_bytes(target_path, new_content.as_bytes())?;
    Ok(("written", backup_path))
}

fn resolve_skill_paths(scope: InstallScope, env: &InstallEnv) -> Result<Vec<PathBuf>, String> {
    match scope {
        InstallScope::User => {
            let home = require_home(env)?;
            Ok(vec![
                home.join(".agents/skills/apexuse/SKILL.md"),
                home.join(".claude/skills/apexuse/SKILL.md"),
                home.join(".codex/skills/apexuse/SKILL.md"),
            ])
        }
        InstallScope::Project => Ok(vec![
            env.project_dir.join(".agents/skills/apexuse/SKILL.md"),
            env.project_dir.join(".claude/skills/apexuse/SKILL.md"),
        ]),
    }
}

fn write_bytes_if_changed(path: &Path, content: &[u8]) -> Result<bool, String> {
    let same = fs::read(path).map(|cur| cur == content).unwrap_or(false);
    if same {
        return Ok(false);
    }
    atomic_write_bytes(path, content)?;
    Ok(true)
}

fn apply_skill_install(
    scope: InstallScope,
    do_write: bool,
    env: &InstallEnv,
) -> Result<(Vec<String>, bool), String> {
    let paths = resolve_skill_paths(scope, env)?;
    let mut written = false;
    if do_write {
        for p in &paths {
            let _ = write_bytes_if_changed(p, SKILL_MD.as_bytes())?;
        }
        written = true;
    }
    Ok((
        paths
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        written,
    ))
}

fn install_mode_str(opts: &InstallOptions) -> &'static str {
    if opts.dry_run {
        "dry_run"
    } else if opts.write {
        "write"
    } else {
        "snippet"
    }
}

fn read_existing_config(target_path: &Path, inspect_existing: bool) -> Result<Option<String>, String> {
    if inspect_existing && target_path.exists() {
        Ok(Some(fs::read_to_string(target_path).map_err(|e| {
            format!("failed to read {}: {e}", target_path.display())
        })?))
    } else {
        Ok(None)
    }
}

fn compute_dry_run_status(existing_raw: Option<&str>, new_content: &str) -> (&'static str, bool) {
    let changed = existing_raw != Some(new_content);
    let will_bak = changed && existing_raw.is_some_and(|s| !s.trim().is_empty());
    let st = if changed { "would_write" } else { "unchanged" };
    (st, will_bak)
}

fn build_install_result(
    agent: &str,
    opts: &InstallOptions,
    status: &str,
    format_name: &str,
    target_path: &Path,
    backup_path: Option<PathBuf>,
    would_backup: bool,
    snippet: String,
    extra_fields: Vec<(&'static str, JsonValue)>,
    env: &InstallEnv,
) -> Result<JsonValue, String> {
    let do_write = opts.write && !opts.dry_run;
    let mut fields = vec![
        ("ok", JsonValue::Bool(true)),
        ("agent", JsonValue::String(agent.to_string())),
        ("scope", JsonValue::String(opts.scope.as_str().into())),
        ("mode", JsonValue::String(install_mode_str(opts).into())),
        ("status", JsonValue::String(status.into())),
        ("format", JsonValue::String(format_name.into())),
        (
            "target_path",
            JsonValue::String(target_path.to_string_lossy().into_owned()),
        ),
        (
            "backup_path",
            match backup_path {
                Some(p) => JsonValue::String(p.to_string_lossy().into_owned()),
                None => JsonValue::Null,
            },
        ),
        ("snippet", JsonValue::String(snippet)),
    ];

    if opts.dry_run {
        fields.push(("would_backup", JsonValue::Bool(would_backup)));
    }
    fields.extend(extra_fields);

    if opts.with_skill {
        let (skill_paths, skill_written) = apply_skill_install(opts.scope, do_write, env)?;
        fields.push((
            "skill_paths",
            JsonValue::Array(skill_paths.into_iter().map(JsonValue::String).collect()),
        ));
        fields.push(("skill_written", JsonValue::Bool(skill_written)));
    }

    Ok(JsonValue::object(fields))
}

fn install_codex_plugin_with_env(
    exe_path: &Path,
    opts: &InstallOptions,
    env: &InstallEnv,
) -> Result<JsonValue, String> {
    let target_path = resolve_target_path(CODEX_PLUGIN_TARGET, opts.scope, env)?;
    let home = require_home(env)?;
    let plugins_root = home.join(".codex").join("plugins");
    let marketplace_path = plugins_root.join("marketplaces").join("apexuse-local");
    let cache_path = plugins_root
        .join("cache")
        .join("apexuse-local")
        .join("apexuse")
        .join(env!("CARGO_PKG_VERSION"));

    let (cmd, args) = resolved_install_invocation(CODEX_PLUGIN_TARGET, exe_path, opts);
    let do_write = opts.write && !opts.dry_run;
    let inspect_existing = opts.write || opts.dry_run;
    let existing_raw = read_existing_config(&target_path, inspect_existing)?;

    let marketplace_block = render_codex_marketplace_toml_block(&marketplace_path);
    let plugin_block = render_codex_plugin_toml_block();
    let snippet_str = format!("{marketplace_block}\n\n{plugin_block}");
    let new_content = if inspect_existing {
        upsert_codex_plugin_toml(
            existing_raw.as_deref(),
            &marketplace_block,
            plugin_block,
            &target_path,
        )?
    } else {
        format!("{snippet_str}\n")
    };

    let mcp_json_content = render_codex_plugin_mcp_json(&cmd, &args, opts);
    let marketplace_plugin_dir = marketplace_path.join("plugins").join("apexuse");
    let asset_files: [(PathBuf, &[u8]); 9] = [
        (
            marketplace_path
                .join(".agents")
                .join("plugins")
                .join("marketplace.json"),
            CODEX_MARKETPLACE_JSON.as_bytes(),
        ),
        (
            marketplace_plugin_dir.join(".codex-plugin").join("plugin.json"),
            CODEX_PLUGIN_JSON.as_bytes(),
        ),
        (
            marketplace_plugin_dir.join(".mcp.json"),
            mcp_json_content.as_bytes(),
        ),
        (
            marketplace_plugin_dir.join("assets").join("apexuse.svg"),
            CODEX_PLUGIN_SVG.as_bytes(),
        ),
        (
            marketplace_plugin_dir
                .join("assets")
                .join("apexuse-small.svg"),
            CODEX_PLUGIN_SMALL_SVG.as_bytes(),
        ),
        (
            cache_path.join(".codex-plugin").join("plugin.json"),
            CODEX_PLUGIN_JSON.as_bytes(),
        ),
        (cache_path.join(".mcp.json"), mcp_json_content.as_bytes()),
        (
            cache_path.join("assets").join("apexuse.svg"),
            CODEX_PLUGIN_SVG.as_bytes(),
        ),
        (
            cache_path.join("assets").join("apexuse-small.svg"),
            CODEX_PLUGIN_SMALL_SVG.as_bytes(),
        ),
    ];

    let (status, backup_path, would_backup) = if do_write {
        let mut any_asset_written = false;
        for (path, bytes) in &asset_files {
            if write_bytes_if_changed(path, bytes)? {
                any_asset_written = true;
            }
        }
        let (toml_status, bak) =
            write_config_safely(&target_path, existing_raw.as_deref(), &new_content)?;
        let combined_status = if toml_status == "written" || any_asset_written {
            "written"
        } else {
            "unchanged"
        };
        (combined_status, bak, false)
    } else if opts.dry_run {
        let (toml_status, will_bak) =
            compute_dry_run_status(existing_raw.as_deref(), &new_content);
        let assets_changed = asset_files
            .iter()
            .any(|(path, bytes)| fs::read(path).ok().as_deref() != Some(*bytes));
        let st = if toml_status == "would_write" || assets_changed {
            "would_write"
        } else {
            "unchanged"
        };
        (st, None, will_bak)
    } else {
        ("preview", None, false)
    };

    build_install_result(
        CODEX_PLUGIN_TARGET,
        opts,
        status,
        "toml",
        &target_path,
        backup_path,
        would_backup,
        snippet_str,
        vec![
            (
                "marketplace_path",
                JsonValue::String(marketplace_path.to_string_lossy().into_owned()),
            ),
            (
                "cache_path",
                JsonValue::String(cache_path.to_string_lossy().into_owned()),
            ),
        ],
        env,
    )
}

pub(crate) fn install_agent_with_env(
    raw_agent: &str,
    exe_path: &Path,
    opts: &InstallOptions,
    env: &InstallEnv,
) -> Result<JsonValue, String> {
    let agent = raw_agent.trim().to_ascii_lowercase();
    if agent == "all" {
        return Err("use install_all for 'all', or specify a concrete agent name".into());
    }
    if agent == CODEX_PLUGIN_TARGET {
        return install_codex_plugin_with_env(exe_path, opts, env);
    }
    if !SUPPORTED_AGENTS.contains(&agent.as_str()) {
        return Err(format!(
            "Unsupported agent '{raw_agent}'. Supported: {}, {}, all",
            SUPPORTED_AGENTS.join(", "),
            CODEX_PLUGIN_TARGET
        ));
    }

    let target_path = resolve_target_path(&agent, opts.scope, env)?;
    let (cmd, args) = resolved_install_invocation(&agent, exe_path, opts);
    let do_write = opts.write && !opts.dry_run;
    let inspect_existing = opts.write || opts.dry_run;
    let existing_raw = read_existing_config(&target_path, inspect_existing)?;

    let (format_name, new_content, config_json_opt, snippet_str) = if agent == "codex" {
        let block = render_codex_toml_block(&cmd, &args, opts);
        let merged = if inspect_existing {
            upsert_codex_toml(
                existing_raw.as_deref(),
                &block,
                opts.scope == InstallScope::User,
                &target_path,
            )?
        } else {
            format!("{block}\n")
        };
        ("toml", merged, None, block)
    } else {
        let container_key = json_container_key(&agent);
        let entry = build_server_entry(&agent, &cmd, &args, opts);

        let mut root_map = if let Some(raw) = existing_raw.as_deref() {
            if raw.trim().is_empty() {
                BTreeMap::new()
            } else {
                let parsed = parse_json(raw).map_err(|e| {
                    format!(
                        "refusing to modify {}: invalid JSON ({e})",
                        target_path.display()
                    )
                })?;
                match parsed {
                    JsonValue::Object(m) => m,
                    _ => {
                        return Err(format!(
                            "refusing to modify {}: JSON root must be a JSON object",
                            target_path.display()
                        ));
                    }
                }
            }
        } else {
            BTreeMap::new()
        };

        let mut container_map = match root_map.remove(container_key) {
            None => BTreeMap::new(),
            Some(JsonValue::Object(m)) => m,
            Some(_) => {
                return Err(format!(
                    "refusing to modify {}: '{}' must be a JSON object",
                    target_path.display(),
                    container_key
                ));
            }
        };
        container_map.insert("apexuse".to_string(), entry);
        root_map.insert(container_key.to_string(), JsonValue::Object(container_map));
        let merged_json = JsonValue::Object(root_map);
        let formatted = format!("{}\n", format_json_pretty(&merged_json));
        let snippet = formatted.trim_end_matches('\n').to_string();
        ("json", formatted, Some(merged_json), snippet)
    };

    let (status, backup_path, would_backup) = if do_write {
        let (st, bak) = write_config_safely(&target_path, existing_raw.as_deref(), &new_content)?;
        (st, bak, false)
    } else if opts.dry_run {
        let (st, will_bak) = compute_dry_run_status(existing_raw.as_deref(), &new_content);
        (st, None, will_bak)
    } else {
        ("preview", None, false)
    };

    let mut extra_fields = Vec::new();
    if let Some(cfg_val) = config_json_opt {
        extra_fields.push(("config", cfg_val));
    }
    if agent == "claude" && !opts.write && !opts.dry_run {
        extra_fields.push((
            "cli_hint",
            JsonValue::String(build_claude_cli_hint(&cmd, &args, opts)),
        ));
    }

    build_install_result(
        &agent,
        opts,
        status,
        format_name,
        &target_path,
        backup_path,
        would_backup,
        snippet_str,
        extra_fields,
        env,
    )
}

fn has_cli_on_path(bin_name: &str, env: &InstallEnv) -> bool {
    let candidates: Vec<String> = if env.os == TargetOs::Windows {
        vec![
            format!("{bin_name}.exe"),
            format!("{bin_name}.cmd"),
            format!("{bin_name}.bat"),
            bin_name.to_string(),
        ]
    } else {
        vec![bin_name.to_string()]
    };
    env.path_dirs.iter().any(|dir| {
        candidates
            .iter()
            .any(|name| dir.join(name).is_file())
    })
}

fn has_app_bundle(bundle_name: &str, env: &InstallEnv) -> bool {
    env.applications_dir
        .as_ref()
        .is_some_and(|dir| dir.join(bundle_name).exists())
}

fn detect_agent(agent: &str, scope: InstallScope, env: &InstallEnv) -> Result<(), &'static str> {
    if scope == InstallScope::Project {
        let p = &env.project_dir;
        let detected = match agent {
            "antigravity" => p.join(".agents").exists(),
            "gemini" => p.join(".gemini").exists(),
            "claude" => p.join(".mcp.json").exists() || p.join(".claude").exists(),
            "claude-desktop" => return Err("project scope unsupported for claude-desktop"),
            "codex" => p.join(".codex").exists(),
            "cursor" => p.join(".cursor").exists(),
            "windsurf" => p.join(".codeium/windsurf").exists() || p.join(".windsurf").exists(),
            "vscode" => p.join(".vscode").exists(),
            "opencode" => p.join("opencode.json").exists() || p.join(".opencode").exists(),
            _ => false,
        };
        return if detected {
            Ok(())
        } else {
            Err("not detected in project directory")
        };
    }

    let Some(home) = env.home.as_deref() else {
        return Err("home directory unavailable");
    };
    let app_data = env
        .app_data
        .clone()
        .unwrap_or_else(|| home.join("AppData").join("Roaming"));
    let xdg = env
        .xdg_config
        .clone()
        .unwrap_or_else(|| home.join(".config"));

    let detected = match agent {
        "antigravity" => {
            home.join(".gemini/antigravity").exists() || has_cli_on_path("antigravity", env)
        }
        "gemini" => {
            home.join(".gemini/settings.json").exists() || has_cli_on_path("gemini", env)
        }
        "claude" => {
            home.join(".claude.json").exists()
                || home.join(".claude").exists()
                || has_cli_on_path("claude", env)
        }
        "claude-desktop" => {
            let dir = match env.os {
                TargetOs::Macos => home.join("Library/Application Support/Claude"),
                TargetOs::Windows => app_data.join("Claude"),
                TargetOs::Linux => xdg.join("Claude"),
            };
            dir.exists() || has_app_bundle("Claude.app", env)
        }
        "codex" => home.join(".codex").exists() || has_cli_on_path("codex", env),
        "cursor" => {
            home.join(".cursor").exists()
                || has_cli_on_path("cursor", env)
                || has_app_bundle("Cursor.app", env)
        }
        "windsurf" => {
            home.join(".codeium/windsurf").exists()
                || has_cli_on_path("windsurf", env)
                || has_app_bundle("Windsurf.app", env)
        }
        "vscode" => {
            let dir = match env.os {
                TargetOs::Macos => home.join("Library/Application Support/Code/User"),
                TargetOs::Windows => app_data.join("Code/User"),
                TargetOs::Linux => xdg.join("Code/User"),
            };
            dir.exists()
                || has_cli_on_path("code", env)
                || has_app_bundle("Visual Studio Code.app", env)
        }
        "opencode" => {
            xdg.join("opencode").exists() || has_cli_on_path("opencode", env)
        }
        _ => false,
    };

    if detected {
        Ok(())
    } else {
        Err("not detected")
    }
}

fn should_preserve_codex_plugin(scope: InstallScope, env: &InstallEnv) -> bool {
    if scope != InstallScope::User {
        return false;
    }
    let Ok(config_path) = resolve_target_path(CODEX_PLUGIN_TARGET, scope, env) else {
        return false;
    };
    let Ok(raw) = fs::read_to_string(&config_path) else {
        return false;
    };
    let Ok(sections) = parse_toml_sections(&raw, &config_path) else {
        return false;
    };
    sections
        .iter()
        .filter(|s| s.table.as_deref().is_some_and(is_codex_plugin_table))
        .any(|s| read_toml_section_bool(&s.lines, "enabled") == Some(true))
}

pub(crate) fn install_all_with_env(
    exe_path: &Path,
    opts: &InstallOptions,
    env: &InstallEnv,
) -> Result<JsonValue, String> {
    if !opts.write && !opts.dry_run {
        return Err("install all requires --write or --dry-run".into());
    }

    let do_write = opts.write && !opts.dry_run;
    let mode = if opts.dry_run { "dry_run" } else { "write" };
    let per_agent_opts = InstallOptions {
        with_skill: false,
        ..opts.clone()
    };

    let mut wired = Vec::new();
    let mut skipped = Vec::new();
    let mut errors = Vec::new();

    for &agent in SUPPORTED_AGENTS {
        if let Err(reason) = detect_agent(agent, opts.scope, env) {
            skipped.push(JsonValue::object(vec![
                ("agent", JsonValue::String(agent.into())),
                ("reason", JsonValue::String(reason.into())),
            ]));
            continue;
        }
        let effective_agent = if agent == "codex" && should_preserve_codex_plugin(opts.scope, env) {
            CODEX_PLUGIN_TARGET
        } else {
            agent
        };
        match install_agent_with_env(effective_agent, exe_path, &per_agent_opts, env) {
            Ok(val) => wired.push(val),
            Err(err) => errors.push(JsonValue::object(vec![
                ("agent", JsonValue::String(effective_agent.into())),
                ("error", JsonValue::String(err)),
            ])),
        }
    }

    let mut fields = vec![
        ("ok", JsonValue::Bool(errors.is_empty())),
        ("scope", JsonValue::String(opts.scope.as_str().into())),
        ("mode", JsonValue::String(mode.into())),
        ("wired", JsonValue::Array(wired)),
        ("skipped", JsonValue::Array(skipped)),
        ("errors", JsonValue::Array(errors)),
    ];

    if opts.with_skill {
        let (skill_paths, skill_written) = apply_skill_install(opts.scope, do_write, env)?;
        fields.push((
            "skill_paths",
            JsonValue::Array(skill_paths.into_iter().map(JsonValue::String).collect()),
        ));
        fields.push(("skill_written", JsonValue::Bool(skill_written)));
    }

    Ok(JsonValue::object(fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_json;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQ: AtomicU64 = AtomicU64::new(1);

    fn make_temp_env(tag: &str, os: TargetOs) -> (PathBuf, InstallEnv) {
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "apexuse_installer_{}_{}_{}",
            tag,
            std::process::id(),
            seq
        ));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let app_data = root.join("appdata");
        let xdg_config = root.join("home").join(".config");
        let project_dir = root.join("project");
        let bin_dir = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&app_data).unwrap();
        fs::create_dir_all(&xdg_config).unwrap();
        fs::create_dir_all(&project_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();

        let env = InstallEnv {
            home: Some(home),
            app_data: Some(app_data),
            xdg_config: Some(xdg_config),
            project_dir,
            path_dirs: vec![bin_dir],
            applications_dir: None,
            os,
        };
        (root, env)
    }

    #[test]
    fn merge_json_preserves_existing_servers_and_creates_backup() {
        let (root, env) = make_temp_env("merge_json", TargetOs::Macos);
        let target = env.home.as_ref().unwrap().join(".cursor/mcp.json");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let original = "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"foo\",\n      \"args\": [\"bar\"]\n    }\n  }\n}\n";
        fs::write(&target, original).unwrap();

        let opts = InstallOptions {
            write: true,
            dry_run: false,
            scope: InstallScope::User,
            use_npx: false,
            policy_path: Some(PathBuf::from("/tmp/policy.autonomous.yaml")),
            with_skill: false,
            ..InstallOptions::default()
        };
        let exe = Path::new("/usr/local/bin/apexuse-mcp");
        let res = install_agent_with_env("cursor", exe, &opts, &env).expect("merge should succeed");
        assert_eq!(res.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(res.get("status").and_then(|v| v.as_str()), Some("written"));

        // .bak exists with exact original content
        let bak = PathBuf::from(format!("{}.bak", target.display()));
        assert!(bak.exists(), "backup file must exist");
        assert_eq!(fs::read_to_string(&bak).unwrap(), original);

        // Updated file is valid pretty JSON preserving "theme" and "other"
        let updated_raw = fs::read_to_string(&target).unwrap();
        assert!(updated_raw.contains('\n'), "written JSON should be pretty-printed");
        let parsed = parse_json(&updated_raw).unwrap();
        assert_eq!(parsed.get("theme").and_then(|v| v.as_str()), Some("dark"));
        let servers = parsed.get("mcpServers").unwrap();
        assert_eq!(
            servers.get("other").and_then(|v| v.get("command")).and_then(|v| v.as_str()),
            Some("foo")
        );
        let apex = servers.get("apexuse").unwrap();
        assert_eq!(apex.get("command").and_then(|v| v.as_str()), Some("/usr/local/bin/apexuse-mcp"));
        assert_eq!(
            apex.get("env").and_then(|v| v.get("APEXUSE_POLICY")).and_then(|v| v.as_str()),
            Some("/tmp/policy.autonomous.yaml")
        );

        // Second run is idempotent ("unchanged") and preserves original .bak
        let res2 = install_agent_with_env("cursor", exe, &opts, &env).expect("idempotent rerun");
        assert_eq!(res2.get("status").and_then(|v| v.as_str()), Some("unchanged"));
        assert_eq!(fs::read_to_string(&bak).unwrap(), original);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_commented_or_corrupt_json_without_touching_disk() {
        let (root, env) = make_temp_env("corrupt_json", TargetOs::Linux);
        let target = env.xdg_config.as_ref().unwrap().join("opencode/opencode.json");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let corrupt = "{\n // comment\n \"mcp\": {}\n}\n";
        fs::write(&target, corrupt).unwrap();

        let opts = InstallOptions {
            write: true,
            dry_run: false,
            scope: InstallScope::User,
            use_npx: false,
            policy_path: None,
            with_skill: false,
            ..InstallOptions::default()
        };
        let err = install_agent_with_env("opencode", Path::new("apexuse-mcp"), &opts, &env)
            .expect_err("commented JSON must fail closed");
        assert!(err.contains("invalid JSON"), "unexpected error: {err}");

        // Original file untouched & no .bak created
        assert_eq!(fs::read_to_string(&target).unwrap(), corrupt);
        let bak = PathBuf::from(format!("{}.bak", target.display()));
        assert!(!bak.exists(), "must not create .bak on invalid JSON");

        // Also reject non-object container ("mcpServers": [])
        let bad_container = env.home.as_ref().unwrap().join(".cursor/mcp.json");
        fs::create_dir_all(bad_container.parent().unwrap()).unwrap();
        let bad_raw = r#"{"mcpServers": []}"#;
        fs::write(&bad_container, bad_raw).unwrap();
        let err2 = install_agent_with_env("cursor", Path::new("apexuse-mcp"), &opts, &env)
            .expect_err("non-object container must fail closed");
        assert!(err2.contains("must be a JSON object"), "unexpected error: {err2}");
        assert_eq!(fs::read_to_string(&bad_container).unwrap(), bad_raw);
        assert!(!PathBuf::from(format!("{}.bak", bad_container.display())).exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn upserts_codex_toml_idempotently_preserving_siblings_and_top_level_keys() {
        let (root, env) = make_temp_env("codex_toml", TargetOs::Macos);
        let target = env.home.as_ref().unwrap().join(".codex/config.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let initial = concat!(
            "model = \"gpt-5\"\n\n",
            "[mcp_servers.apexuse]\n",
            "command = \"/old/apexuse-mcp\"\n",
            "args = []\n\n",
            "[mcp_servers.apexuse.env]\n",
            "APEXUSE_POLICY = \"/old/policy.yaml\"\n\n",
            "[mcp_servers.context7]\n",
            "command = \"npx\"\n",
            "args = [\"-y\", \"@upstash/context7-mcp\"]"
        );
        fs::write(&target, initial).unwrap();

        let opts = InstallOptions {
            write: true,
            dry_run: false,
            scope: InstallScope::User,
            use_npx: true,
            policy_path: Some(PathBuf::from("/new/policy.yaml")),
            with_skill: false,
            ..InstallOptions::default()
        };
        let res = install_agent_with_env("codex", Path::new("/unused"), &opts, &env)
            .expect("codex upsert should succeed");
        assert_eq!(res.get("ok").and_then(|v| v.as_bool()), Some(true));

        let first = fs::read_to_string(&target).unwrap();
        assert!(first.contains("model = \"gpt-5\""), "top-level key lost: {first}");
        assert!(first.contains("[mcp_servers.context7]"), "sibling table lost: {first}");
        assert!(first.contains("\"@upstash/context7-mcp\""), "sibling args lost: {first}");
        assert!(!first.contains("/old/apexuse-mcp"), "old command should be replaced: {first}");
        assert!(!first.contains("/old/policy.yaml"), "old policy should be replaced: {first}");
        assert!(first.contains("command = \"npx\""));
        assert!(first.contains("args = [\"-y\", \"apexuse-mcp\"]"));
        assert!(first.contains("APEXUSE_POLICY = \"/new/policy.yaml\""));
        assert_eq!(first.matches("[mcp_servers.apexuse]").count(), 1);
        assert_eq!(first.matches("[mcp_servers.apexuse.env]").count(), 1);

        // Re-running must be byte-for-byte identical
        let res2 = install_agent_with_env("codex", Path::new("/unused"), &opts, &env).unwrap();
        assert_eq!(res2.get("status").and_then(|v| v.as_str()), Some("unchanged"));
        assert_eq!(fs::read_to_string(&target).unwrap(), first);

        // Running without --policy removes [mcp_servers.apexuse.env] cleanly
        let opts_no_policy = InstallOptions {
            policy_path: None,
            use_npx: false,
            ..opts.clone()
        };
        install_agent_with_env("codex", Path::new(r"C:\Tools\apexuse-mcp.exe"), &opts_no_policy, &env)
            .unwrap();
        let after_no_policy = fs::read_to_string(&target).unwrap();
        assert!(!after_no_policy.contains("[mcp_servers.apexuse.env]"));
        assert!(after_no_policy.contains(r#"command = "C:\\Tools\\apexuse-mcp.exe""#));
        assert!(after_no_policy.contains("[mcp_servers.context7]"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn formats_opencode_vscode_claude_and_npx_cleanly() {
        let (root, env_mac) = make_temp_env("formats_mac", TargetOs::Macos);
        let opts_npx = InstallOptions {
            write: false,
            dry_run: true,
            scope: InstallScope::User,
            use_npx: true,
            policy_path: Some(PathBuf::from("/etc/apexuse/policy.yaml")),
            with_skill: false,
            ..InstallOptions::default()
        };

        // OpenCode
        let oc = install_agent_with_env("opencode", Path::new("apexuse-mcp"), &opts_npx, &env_mac)
            .unwrap();
        let oc_cfg = oc.get("config").unwrap();
        let oc_entry = oc_cfg.get("mcp").unwrap().get("apexuse").unwrap();
        assert_eq!(oc_entry.get("type").and_then(|v| v.as_str()), Some("local"));
        assert_eq!(oc_entry.get("enabled").and_then(|v| v.as_bool()), Some(true));
        let oc_cmd = oc_entry.get("command").and_then(|v| v.as_array()).unwrap();
        assert_eq!(
            oc_cmd.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
            vec!["npx", "-y", "apexuse-mcp"]
        );
        assert_eq!(
            oc_entry
                .get("environment")
                .and_then(|e| e.get("APEXUSE_POLICY"))
                .and_then(|v| v.as_str()),
            Some("/etc/apexuse/policy.yaml")
        );

        // VS Code (User macOS vs Project)
        let vsc_user = install_agent_with_env("vscode", Path::new("/bin/apex"), &opts_npx, &env_mac)
            .unwrap();
        assert!(vsc_user
            .get("target_path")
            .and_then(|v| v.as_str())
            .unwrap()
            .replace('\\', "/")
            .ends_with("Library/Application Support/Code/User/mcp.json"));
        let vsc_entry = vsc_user
            .get("config")
            .unwrap()
            .get("servers")
            .unwrap()
            .get("apexuse")
            .unwrap();
        assert_eq!(vsc_entry.get("type").and_then(|v| v.as_str()), Some("stdio"));
        assert_eq!(vsc_entry.get("command").and_then(|v| v.as_str()), Some("npx"));
        let vsc_args = vsc_entry.get("args").and_then(|v| v.as_array()).unwrap();
        assert_eq!(
            vsc_args.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
            vec!["-y", "apexuse-mcp"]
        );

        // Claude bare snippet mode includes cli_hint
        let opts_bare = InstallOptions {
            write: false,
            dry_run: false,
            scope: InstallScope::User,
            use_npx: true,
            policy_path: Some(PathBuf::from("/p.yaml")),
            with_skill: false,
            ..InstallOptions::default()
        };
        let claude_bare = install_agent_with_env("claude", Path::new("/bin/apex"), &opts_bare, &env_mac)
            .unwrap();
        let hint = claude_bare.get("cli_hint").and_then(|v| v.as_str()).unwrap();
        assert!(hint.contains("claude mcp add"), "missing cli_hint: {hint}");
        assert!(hint.contains("npx -y apexuse-mcp"));
        assert!(hint.contains("APEXUSE_POLICY=/p.yaml"));

        // Claude Desktop user paths across OSes & project error
        let (root_win, env_win) = make_temp_env("formats_win", TargetOs::Windows);
        let cd_win = install_agent_with_env("claude-desktop", Path::new("apex.exe"), &opts_bare, &env_win)
            .unwrap();
        assert!(cd_win
            .get("target_path")
            .and_then(|v| v.as_str())
            .unwrap()
            .replace('\\', "/")
            .ends_with("appdata/Claude/claude_desktop_config.json"));

        let opts_proj = InstallOptions {
            scope: InstallScope::Project,
            ..opts_npx.clone()
        };
        assert!(install_agent_with_env("claude-desktop", Path::new("apex"), &opts_proj, &env_mac).is_err());

        // Dry-run must NOT write target file
        let cursor_target = env_mac.home.as_ref().unwrap().join(".cursor/mcp.json");
        assert!(!cursor_target.exists());
        install_agent_with_env("cursor", Path::new("/bin/apex"), &opts_npx, &env_mac).unwrap();
        assert!(!cursor_target.exists());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&root_win);
    }

    #[test]
    fn with_skill_writes_embedded_skill_md_only_on_write() {
        let (root, env) = make_temp_env("with_skill", TargetOs::Macos);

        // Dry run must not write skill files
        let dry_opts = InstallOptions {
            write: false,
            dry_run: true,
            scope: InstallScope::User,
            use_npx: false,
            policy_path: None,
            with_skill: true,
            ..InstallOptions::default()
        };
        let dry_res = install_agent_with_env("claude", Path::new("/bin/apex"), &dry_opts, &env).unwrap();
        assert_eq!(dry_res.get("skill_written").and_then(|v| v.as_bool()), Some(false));
        let home = env.home.as_ref().unwrap();
        assert!(!home.join(".agents/skills/apexuse/SKILL.md").exists());

        // Write mode writes embedded SKILL_MD
        let write_opts = InstallOptions {
            write: true,
            dry_run: false,
            ..dry_opts
        };
        let write_res = install_agent_with_env("claude", Path::new("/bin/apex"), &write_opts, &env).unwrap();
        assert_eq!(write_res.get("skill_written").and_then(|v| v.as_bool()), Some(true));
        for rel in [
            ".agents/skills/apexuse/SKILL.md",
            ".claude/skills/apexuse/SKILL.md",
            ".codex/skills/apexuse/SKILL.md",
        ] {
            let p = home.join(rel);
            assert!(p.exists(), "missing skill file {}", p.display());
            assert_eq!(fs::read_to_string(&p).unwrap(), SKILL_MD);
        }

        // Project scope writes project skill dirs
        let proj_opts = InstallOptions {
            scope: InstallScope::Project,
            ..write_opts
        };
        install_agent_with_env("cursor", Path::new("/bin/apex"), &proj_opts, &env).unwrap();
        for rel in [
            ".agents/skills/apexuse/SKILL.md",
            ".claude/skills/apexuse/SKILL.md",
        ] {
            let p = env.project_dir.join(rel);
            assert!(p.exists(), "missing project skill file {}", p.display());
            assert_eq!(fs::read_to_string(&p).unwrap(), SKILL_MD);
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_all_requires_write_or_dry_run_and_detects_installed_agents() {
        let (root, env) = make_temp_env("install_all", TargetOs::Macos);
        let bare = InstallOptions::default();
        assert!(install_all_with_env(Path::new("/bin/apex"), &bare, &env).is_err());

        // Mark cursor dir and codex dir + claude binary in fake PATH
        let home = env.home.as_ref().unwrap();
        fs::create_dir_all(home.join(".cursor")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        let claude_bin = env.path_dirs[0].join("claude");
        fs::write(&claude_bin, "#!/bin/sh\n").unwrap();

        let opts = InstallOptions {
            write: true,
            dry_run: false,
            scope: InstallScope::User,
            use_npx: true,
            policy_path: None,
            with_skill: true,
            ..InstallOptions::default()
        };
        let res = install_all_with_env(Path::new("/bin/apex"), &opts, &env).unwrap();
        assert_eq!(res.get("ok").and_then(|v| v.as_bool()), Some(true));

        let wired = res.get("wired").and_then(|v| v.as_array()).unwrap();
        let wired_agents: Vec<&str> = wired
            .iter()
            .filter_map(|item| item.get("agent").and_then(|v| v.as_str()))
            .collect();
        assert!(wired_agents.contains(&"cursor"));
        assert!(wired_agents.contains(&"codex"));
        assert!(wired_agents.contains(&"claude"));
        assert!(!wired_agents.contains(&"windsurf"));

        let skipped = res.get("skipped").and_then(|v| v.as_array()).unwrap();
        assert!(skipped
            .iter()
            .any(|item| item.get("agent").and_then(|v| v.as_str()) == Some("windsurf")));

        assert!(home.join(".cursor/mcp.json").exists());
        assert!(home.join(".codex/config.toml").exists());
        assert!(home.join(".claude.json").exists());
        assert!(home.join(".agents/skills/apexuse/SKILL.md").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn handles_opencode_without_env_and_codex_multiline_strings_and_partial_install_all_errors() {
        let (root, env) = make_temp_env("edge_cases", TargetOs::Linux);
        let opts_no_policy = InstallOptions {
            write: true,
            dry_run: false,
            scope: InstallScope::User,
            use_npx: false,
            policy_path: None,
            with_skill: false,
            ..InstallOptions::default()
        };

        // OpenCode without --policy omits "environment"
        let oc = install_agent_with_env("opencode", Path::new("/opt/apexuse-mcp"), &opts_no_policy, &env)
            .unwrap();
        let oc_entry = oc.get("config").unwrap().get("mcp").unwrap().get("apexuse").unwrap();
        assert!(oc_entry.get("environment").is_none());
        let oc_cmd = oc_entry.get("command").and_then(|v| v.as_array()).unwrap();
        assert_eq!(
            oc_cmd.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
            vec!["/opt/apexuse-mcp"]
        );

        // Codex multiline string containing literal "[mcp_servers.apexuse]" is NOT mistaken for a section
        let codex_path = env.home.as_ref().unwrap().join(".codex/config.toml");
        fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
        let tricky_toml = concat!(
            "notes = \"\"\"\n",
            "[mcp_servers.apexuse]\n",
            "not a section\n",
            "\"\"\"\n\n",
            "[mcp_servers.context7]\n",
            "command = \"npx\"\n"
        );
        fs::write(&codex_path, tricky_toml).unwrap();
        install_agent_with_env("codex", Path::new("/opt/apexuse-mcp"), &opts_no_policy, &env).unwrap();
        let updated_toml = fs::read_to_string(&codex_path).unwrap();
        assert!(updated_toml.contains("not a section"));
        assert!(updated_toml.contains("[mcp_servers.context7]"));
        assert!(updated_toml.contains("command = \"/opt/apexuse-mcp\""));

        // install_all reports corrupt detected agent in errors without touching valid ones
        let cursor_dir = env.home.as_ref().unwrap().join(".cursor");
        fs::create_dir_all(&cursor_dir).unwrap();
        fs::write(cursor_dir.join("mcp.json"), "{ // broken\n}").unwrap();
        let all_res = install_all_with_env(Path::new("/opt/apexuse-mcp"), &opts_no_policy, &env).unwrap();
        assert_eq!(all_res.get("ok").and_then(|v| v.as_bool()), Some(false));
        let errors = all_res.get("errors").and_then(|v| v.as_array()).unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].get("agent").and_then(|v| v.as_str()), Some("cursor"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_plugin_upserts_marketplace_materializes_cache_and_clears_mcp_server() {
        let (root, env) = make_temp_env("codex_plugin_upsert", TargetOs::Macos);
        let home = env.home.as_ref().unwrap();
        let codex_toml = home.join(".codex/config.toml");
        fs::create_dir_all(codex_toml.parent().unwrap()).unwrap();
        let initial = concat!(
            "model = \"gpt-5\"\n",
            "notes = \"\"\"\n",
            "[mcp_servers.apexuse]\n",
            "[plugins.\"apexuse@apexuse-local\"]\n",
            "\"\"\"\n\n",
            "[mcp_servers.apexuse]\n",
            "command = \"/old/apexuse-mcp\"\n",
            "args = []\n\n",
            "[mcp_servers.apexuse.env]\n",
            "APEXUSE_POLICY = \"/old/policy.yaml\"\n\n",
            "[mcp_servers.context7]\n",
            "command = \"npx\"\n\n",
            "[plugins.\"other@marketplace\"]\n",
            "enabled = true\n"
        );
        fs::write(&codex_toml, initial).unwrap();

        let dry_opts = InstallOptions {
            dry_run: true,
            use_npx: true,
            policy_path: Some(PathBuf::from("/new/policy.yaml")),
            with_skill: true,
            ..InstallOptions::default()
        };
        let dry_res =
            install_agent_with_env(CODEX_PLUGIN_TARGET, Path::new("/bin/apex"), &dry_opts, &env)
                .expect("dry run should succeed");
        assert_eq!(dry_res.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            dry_res.get("agent").and_then(|v| v.as_str()),
            Some(CODEX_PLUGIN_TARGET)
        );
        assert_eq!(dry_res.get("mode").and_then(|v| v.as_str()), Some("dry_run"));
        assert_eq!(
            dry_res.get("status").and_then(|v| v.as_str()),
            Some("would_write")
        );
        assert_eq!(dry_res.get("format").and_then(|v| v.as_str()), Some("toml"));
        assert_eq!(
            dry_res.get("would_backup").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            dry_res.get("skill_written").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(!home.join(".codex/plugins").exists());
        assert_eq!(fs::read_to_string(&codex_toml).unwrap(), initial);

        let write_opts = InstallOptions {
            write: true,
            dry_run: false,
            ..dry_opts.clone()
        };
        let write_res =
            install_agent_with_env(CODEX_PLUGIN_TARGET, Path::new("/bin/apex"), &write_opts, &env)
                .expect("write should succeed");
        assert_eq!(write_res.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            write_res.get("status").and_then(|v| v.as_str()),
            Some("written")
        );
        assert_eq!(
            write_res.get("skill_written").and_then(|v| v.as_bool()),
            Some(true)
        );

        let bak = PathBuf::from(format!("{}.bak", codex_toml.display()));
        assert_eq!(fs::read_to_string(&bak).unwrap(), initial);

        let updated = fs::read_to_string(&codex_toml).unwrap();
        assert!(updated.contains("model = \"gpt-5\""));
        assert!(updated.contains("notes = \"\"\""));
        assert!(updated.contains("[mcp_servers.context7]"));
        assert!(updated.contains("[plugins.\"other@marketplace\"]"));
        assert!(!updated.contains("/old/apexuse-mcp"));
        assert!(!updated.contains("[mcp_servers.apexuse.env]"));
        assert_eq!(updated.matches("[mcp_servers.apexuse]").count(), 1);
        assert!(updated.contains("[marketplaces.apexuse-local]"));
        assert!(updated.contains("source_type = \"local\""));
        assert!(updated.contains("[plugins.\"apexuse@apexuse-local\"]"));
        assert!(updated.contains("enabled = true"));

        let marketplace_dir = home
            .join(".codex")
            .join("plugins")
            .join("marketplaces")
            .join("apexuse-local");
        let cache_dir = home
            .join(".codex")
            .join("plugins")
            .join("cache")
            .join("apexuse-local")
            .join("apexuse")
            .join("0.3.0");
        assert_eq!(
            write_res.get("marketplace_path").and_then(|v| v.as_str()),
            Some(marketplace_dir.to_string_lossy().as_ref())
        );
        assert_eq!(
            write_res.get("cache_path").and_then(|v| v.as_str()),
            Some(cache_dir.to_string_lossy().as_ref())
        );

        assert_eq!(
            fs::read_to_string(marketplace_dir.join(".agents/plugins/marketplace.json")).unwrap(),
            CODEX_MARKETPLACE_JSON
        );
        for base in [marketplace_dir.join("plugins/apexuse"), cache_dir.clone()] {
            assert_eq!(
                fs::read_to_string(base.join(".codex-plugin/plugin.json")).unwrap(),
                CODEX_PLUGIN_JSON
            );
            assert_eq!(
                fs::read_to_string(base.join("assets/apexuse.svg")).unwrap(),
                CODEX_PLUGIN_SVG
            );
            assert_eq!(
                fs::read_to_string(base.join("assets/apexuse-small.svg")).unwrap(),
                CODEX_PLUGIN_SMALL_SVG
            );
            let mcp = parse_json(&fs::read_to_string(base.join(".mcp.json")).unwrap()).unwrap();
            let entry = mcp.get("mcpServers").unwrap().get("apexuse").unwrap();
            assert_eq!(entry.get("command").and_then(|v| v.as_str()), Some("npx"));
            let args: Vec<&str> = entry
                .get("args")
                .and_then(|v| v.as_array())
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str())
                .collect();
            assert_eq!(args, vec!["-y", "apexuse-mcp"]);
            assert_eq!(
                entry
                    .get("env")
                    .and_then(|e| e.get("APEXUSE_POLICY"))
                    .and_then(|v| v.as_str()),
                Some("/new/policy.yaml")
            );
        }

        // Idempotent rerun reports unchanged without replacing backup
        let rerun =
            install_agent_with_env(CODEX_PLUGIN_TARGET, Path::new("/bin/apex"), &write_opts, &env)
                .unwrap();
        assert_eq!(rerun.get("status").and_then(|v| v.as_str()), Some("unchanged"));
        assert!(rerun.get("backup_path").is_some_and(|v| v.is_null()));

        // Reconfigure without --npx or --policy updates .mcp.json and omits env
        let bin_opts = InstallOptions {
            write: true,
            use_npx: false,
            policy_path: None,
            ..InstallOptions::default()
        };
        assert_eq!(
            resolved_install_invocation(
                CODEX_PLUGIN_TARGET,
                Path::new("/usr/local/bin/apexuse-mcp"),
                &bin_opts
            ),
            ("/usr/local/bin/apexuse-mcp".to_string(), Vec::new())
        );
        let bin_res = install_agent_with_env(
            CODEX_PLUGIN_TARGET,
            Path::new("/usr/local/bin/apexuse-mcp"),
            &bin_opts,
            &env,
        )
        .unwrap();
        assert_eq!(bin_res.get("status").and_then(|v| v.as_str()), Some("written"));
        let cache_mcp =
            parse_json(&fs::read_to_string(cache_dir.join(".mcp.json")).unwrap()).unwrap();
        let cache_entry = cache_mcp.get("mcpServers").unwrap().get("apexuse").unwrap();
        assert_eq!(
            cache_entry.get("command").and_then(|v| v.as_str()),
            Some("/usr/local/bin/apexuse-mcp")
        );
        assert!(cache_entry.get("env").is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_plugin_rejects_project_scope_and_install_all_preserves_active_plugin() {
        let (root, env) = make_temp_env("codex_plugin_switch", TargetOs::Macos);
        let home = env.home.as_ref().unwrap();

        let proj_opts = InstallOptions {
            write: true,
            scope: InstallScope::Project,
            ..InstallOptions::default()
        };
        let err = install_agent_with_env(
            CODEX_PLUGIN_TARGET,
            Path::new("/usr/local/bin/apexuse-mcp"),
            &proj_opts,
            &env,
        )
        .expect_err("project scope must be rejected");
        assert_eq!(err, "codex-plugin only supports --scope user");

        // Install codex-plugin in user scope
        let plugin_opts = InstallOptions {
            write: true,
            use_npx: true,
            ..InstallOptions::default()
        };
        install_agent_with_env(
            CODEX_PLUGIN_TARGET,
            Path::new("/usr/local/bin/apexuse-mcp"),
            &plugin_opts,
            &env,
        )
        .unwrap();

        // Add another key in [plugins."apexuse@apexuse-local"] and sibling plugin to check preservation
        let codex_toml = home.join(".codex/config.toml");
        let seeded = format!(
            "{}\npinned_note = \"keep\"\n\n[plugins.\"other@other\"]\nenabled = true\n",
            fs::read_to_string(&codex_toml).unwrap().trim_end()
        );
        fs::write(&codex_toml, &seeded).unwrap();

        // install all refreshes codex-plugin instead of overwriting with [mcp_servers.apexuse]
        let all_res = install_all_with_env(
            Path::new("/opt/apexuse-mcp"),
            &InstallOptions {
                write: true,
                use_npx: false,
                ..InstallOptions::default()
            },
            &env,
        )
        .unwrap();
        let wired_agents: Vec<&str> = all_res
            .get("wired")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .filter_map(|item| item.get("agent").and_then(|v| v.as_str()))
            .collect();
        assert!(wired_agents.contains(&CODEX_PLUGIN_TARGET));
        assert!(!wired_agents.contains(&"codex"));
        let after_all = fs::read_to_string(&codex_toml).unwrap();
        assert!(!after_all.contains("[mcp_servers.apexuse]"));
        assert!(after_all.contains("[plugins.\"apexuse@apexuse-local\"]"));
        assert!(after_all.contains("enabled = true"));
        let cache_mcp = parse_json(
            &fs::read_to_string(
                home.join(".codex/plugins/cache/apexuse-local/apexuse/0.3.0/.mcp.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            cache_mcp
                .get("mcpServers")
                .and_then(|s| s.get("apexuse"))
                .and_then(|e| e.get("command"))
                .and_then(|v| v.as_str()),
            Some("/opt/apexuse-mcp")
        );

        // Switching back explicitly to `codex` disables [plugins."apexuse@apexuse-local"] cleanly
        let with_extra = after_all.replace(
            "[plugins.\"apexuse@apexuse-local\"]\nenabled = true",
            "[plugins.\"apexuse@apexuse-local\"]\nenabled = true\npinned_note = \"keep\"",
        );
        fs::write(&codex_toml, with_extra).unwrap();
        install_agent_with_env(
            "codex",
            Path::new("/opt/apexuse-mcp"),
            &InstallOptions {
                write: true,
                ..InstallOptions::default()
            },
            &env,
        )
        .unwrap();
        let after_codex = fs::read_to_string(&codex_toml).unwrap();
        assert!(after_codex.contains("[mcp_servers.apexuse]"));
        assert!(after_codex.contains("pinned_note = \"keep\""));
        assert!(after_codex.contains("enabled = false"));
        assert!(after_codex.contains("[plugins.\"other@other\"]"));

        // Now install all switches to direct `codex` instead of `codex-plugin`
        let all_res2 = install_all_with_env(
            Path::new("/opt/apexuse-mcp"),
            &InstallOptions {
                write: true,
                ..InstallOptions::default()
            },
            &env,
        )
        .unwrap();
        let wired_agents2: Vec<&str> = all_res2
            .get("wired")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .filter_map(|item| item.get("agent").and_then(|v| v.as_str()))
            .collect();
        assert!(wired_agents2.contains(&"codex"));
        assert!(!wired_agents2.contains(&CODEX_PLUGIN_TARGET));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_plugin_escapes_windows_paths_in_toml_and_mcp_json() {
        let (root, env) = make_temp_env("codex_plugin_win", TargetOs::Windows);
        let exe = Path::new(r"C:\Program Files\ApexUse\apexuse-mcp.exe");
        let policy = PathBuf::from(r"C:\Users\alice\My Policies\policy.yaml");
        let opts = InstallOptions {
            write: true,
            policy_path: Some(policy),
            ..InstallOptions::default()
        };

        let res = install_agent_with_env(CODEX_PLUGIN_TARGET, exe, &opts, &env).unwrap();
        assert_eq!(res.get("ok").and_then(|v| v.as_bool()), Some(true));

        let cache_mcp_path = env
            .home
            .as_ref()
            .unwrap()
            .join(".codex/plugins/cache/apexuse-local/apexuse/0.3.0/.mcp.json");
        let mcp_raw = fs::read_to_string(&cache_mcp_path).unwrap();
        assert!(mcp_raw.contains(r#"C:\\Program Files\\ApexUse\\apexuse-mcp.exe"#));
        assert!(mcp_raw.contains(r#"C:\\Users\\alice\\My Policies\\policy.yaml"#));
        let mcp_parsed = parse_json(&mcp_raw).unwrap();
        let entry = mcp_parsed.get("mcpServers").unwrap().get("apexuse").unwrap();
        assert_eq!(
            entry.get("command").and_then(|v| v.as_str()),
            Some(r"C:\Program Files\ApexUse\apexuse-mcp.exe")
        );
        assert_eq!(
            entry
                .get("env")
                .and_then(|e| e.get("APEXUSE_POLICY"))
                .and_then(|v| v.as_str()),
            Some(r"C:\Users\alice\My Policies\policy.yaml")
        );

        let win_marketplace = Path::new(r"C:\Users\alice\.codex\plugins\marketplaces\apexuse-local");
        let rendered = render_codex_marketplace_toml_block(win_marketplace);
        assert!(rendered.contains(
            r#"source = "C:\\Users\\alice\\.codex\\plugins\\marketplaces\\apexuse-local""#
        ));
        let merged = upsert_codex_plugin_toml(
            None,
            &rendered,
            render_codex_plugin_toml_block(),
            Path::new("config.toml"),
        )
        .unwrap();
        assert!(merged.contains(
            r#"source = "C:\\Users\\alice\\.codex\\plugins\\marketplaces\\apexuse-local""#
        ));
        assert!(parse_toml_sections(&merged, Path::new("config.toml")).is_ok());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_options_defaults_and_resolved_invocation() {
        let defaults = InstallOptions::default();
        assert!(!defaults.probe);
        assert_eq!(defaults.probe_timeout_ms, 10_000);

        let exe = Path::new("/opt/bin/apexuse-mcp");
        assert_eq!(
            resolved_install_invocation("codex", exe, &defaults),
            ("/opt/bin/apexuse-mcp".to_string(), Vec::new())
        );
        let npx_opts = InstallOptions {
            use_npx: true,
            ..InstallOptions::default()
        };
        assert_eq!(
            resolved_install_invocation("codex", exe, &npx_opts),
            ("npx".to_string(), vec!["-y".to_string(), "apexuse-mcp".to_string()])
        );
    }

    #[test]
    fn codex_and_codex_plugin_fail_closed_on_corrupt_toml() {
        let (root, env) = make_temp_env("codex_corrupt_toml", TargetOs::Macos);
        let home = env.home.as_ref().unwrap();
        let codex_toml = home.join(".codex/config.toml");
        let bak_path = append_bak_path(&codex_toml);
        let plugins_dir = home.join(".codex/plugins");
        fs::create_dir_all(codex_toml.parent().unwrap()).unwrap();

        let write_opts = InstallOptions {
            write: true,
            use_npx: true,
            ..InstallOptions::default()
        };
        let dry_opts = InstallOptions {
            dry_run: true,
            use_npx: true,
            ..InstallOptions::default()
        };

        let corrupt_samples = [
            "[broken\ncommand = \"npx\"\n",
            "model = \"gpt-5\"\nnotes = \"\"\"\nunclosed multiline\n",
        ];

        for corrupt in corrupt_samples {
            fs::write(&codex_toml, corrupt).unwrap();

            for opts in [&dry_opts, &write_opts] {
                let err_plugin = install_agent_with_env(
                    CODEX_PLUGIN_TARGET,
                    Path::new("/bin/apex"),
                    opts,
                    &env,
                )
                .expect_err("codex-plugin must fail closed on corrupt TOML");
                assert!(
                    err_plugin.contains("refusing to modify"),
                    "unexpected codex-plugin error: {err_plugin}"
                );
                assert_eq!(fs::read(&codex_toml).unwrap(), corrupt.as_bytes());
                assert!(!bak_path.exists());
                assert!(!plugins_dir.exists());

                let err_codex =
                    install_agent_with_env("codex", Path::new("/bin/apex"), opts, &env)
                        .expect_err("codex must fail closed on corrupt TOML");
                assert!(
                    err_codex.contains("refusing to modify"),
                    "unexpected codex error: {err_codex}"
                );
                assert_eq!(fs::read(&codex_toml).unwrap(), corrupt.as_bytes());
                assert!(!bak_path.exists());
                assert!(!plugins_dir.exists());
            }

            let all_res = install_all_with_env(Path::new("/bin/apex"), &write_opts, &env)
                .expect("install_all returns structured report on agent error");
            assert_eq!(all_res.get("ok").and_then(|v| v.as_bool()), Some(false));
            let errors = all_res
                .get("errors")
                .and_then(|v| v.as_array())
                .expect("errors array");
            assert!(
                errors.iter().any(|e| {
                    e.get("agent").and_then(|v| v.as_str()) == Some("codex")
                        && e.get("error")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| s.contains("refusing to modify"))
                }),
                "expected codex fail-closed error in install_all errors: {errors:?}"
            );
            let wired = all_res
                .get("wired")
                .and_then(|v| v.as_array())
                .expect("wired array");
            assert!(!wired.iter().any(|w| {
                matches!(
                    w.get("agent").and_then(|v| v.as_str()),
                    Some("codex" | "codex-plugin")
                )
            }));
            assert_eq!(fs::read(&codex_toml).unwrap(), corrupt.as_bytes());
            assert!(!bak_path.exists());
            assert!(!plugins_dir.exists());
        }

        let _ = fs::remove_dir_all(&root);
    }
}
