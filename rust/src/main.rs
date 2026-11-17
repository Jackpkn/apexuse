use apexuse_mcp::config::{resolve_config_path, AppConfig};
use apexuse_mcp::driver::{default_driver, OsDriver};
use apexuse_mcp::gateway::{Gateway, MockDriver};
use apexuse_mcp::installer::{
    format_json_pretty, install_agent, install_all, resolved_install_invocation, InstallOptions,
    InstallScope, CODEX_PLUGIN_TARGET, SUPPORTED_AGENTS,
};
use apexuse_mcp::json::{parse_json, JsonValue};
use apexuse_mcp::mcp::run_stdio_loop;
use apexuse_mcp::policy::PolicySettings;
use apexuse_mcp::probe::probe_mcp_server;
use apexuse_mcp::som_png::base64_decode;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

fn first_non_empty_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| env::var(k).ok().filter(|v| !v.is_empty()))
}

fn resolve_state_dir() -> PathBuf {
    if let Some(custom) = first_non_empty_env(&["APEXUSE_STATE_DIR", "HANDSMCP_STATE_DIR"]) {
        return PathBuf::from(custom);
    }
    if let Ok(home) = env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".apexuse");
        }
    }
    #[cfg(target_os = "windows")]
    if let Ok(profile) = env::var("USERPROFILE") {
        if !profile.is_empty() {
            return PathBuf::from(profile).join(".apexuse");
        }
    }
    PathBuf::from(".apexuse")
}

fn resolve_policy_path() -> Option<PathBuf> {
    if let Some(custom) = first_non_empty_env(&["APEXUSE_POLICY", "HANDSMCP_POLICY"]) {
        let p = PathBuf::from(custom);
        if p.exists() {
            return Some(p);
        }
    }
    let cwd_policy = PathBuf::from("policy.yaml");
    if cwd_policy.exists() {
        return Some(cwd_policy);
    }
    if let Ok(exe) = env::current_exe() {
        for ancestor in exe.ancestors() {
            let candidate = ancestor.join("policy.yaml");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn fail_json(code: &str, message: &str) -> ! {
    let err = JsonValue::object(vec![
        ("ok", JsonValue::Bool(false)),
        ("code", JsonValue::String(code.into())),
        ("message", JsonValue::String(message.into())),
    ]);
    eprintln!("{}", err.to_json_string());
    process::exit(1);
}

fn print_usage() {
    println!(
        "apexuse-mcp {} — Cross-platform Hybrid Accessibility + Set-of-Mark Visual Indexing MCP\n\
         Usage:\n\
         \x20 apexuse-mcp [--mock]                       Start MCP stdio server\n\
         \x20 apexuse-mcp --version | -V                 Print version\n\
         \x20 apexuse-mcp doctor [--onboard] [--force] [--no-auto-close] [--check] [--mock]\n\
         \x20                                              Check OS readiness or open macOS permission setup\n\
         \x20 apexuse-mcp onboard [--force] [--no-auto-close] [--check] [--mock]\n\
         \x20                                              Open guided macOS permission onboarding window\n\
         \x20 apexuse-mcp probe [--mock] [--npx] [--policy <path>] [--timeout-ms <n>]\n\
         \x20                                              Run live MCP initialize + tools/list handshake probe\n\
         \x20 apexuse-mcp install <agent|{}|all> [flags]  Print or auto-merge MCP client/plugin config\n\
         \x20     --write                                Merge safely into target config (creates .bak)\n\
         \x20     --dry-run                              Preview merged config without writing\n\
         \x20     --scope <user|project>                 Config scope (default: user)\n\
         \x20     --npx                                  Use `npx -y apexuse-mcp` instead of binary path\n\
         \x20     --policy <path>                        Inject APEXUSE_POLICY env var into client config\n\
         \x20     --with-skill                           Install bundled SKILL.md into agent skill directories\n\
         \x20     --probe                                Run live MCP handshake probe after install\n\
         \x20     --timeout-ms <n>                       Probe timeout in milliseconds (default: 10000)\n\
         \x20     --mock                                 Use --mock for temporary --probe invocation only\n\
         \x20 apexuse-mcp config [show|get|set|reset]    Manage persistent runtime defaults\n\
         \x20 apexuse-mcp call <tool> [flags]            Invoke a single MCP tool from the CLI\n\
         \x20 apexuse-mcp call (--calls <json> | --calls-file <path>) [flags]\n\
         \x20     --args '<json>'                        JSON object arguments for single tool call\n\
         \x20     --sleep-ms <n>                         Delay in milliseconds between --calls steps\n\
         \x20     --save-image <path>                    Decode and write returned image to <path>\n\
         \x20     --mock                                 Use deterministic offline MockDriver\n\
         \n\
         Supported install targets: {}, {}, all",
        env!("CARGO_PKG_VERSION"),
        CODEX_PLUGIN_TARGET,
        SUPPORTED_AGENTS.join(", "),
        CODEX_PLUGIN_TARGET
    );
}

fn build_gateway(use_mock: bool, strict_config: bool) -> Gateway {
    let settings = resolve_policy_path()
        .and_then(|p| PolicySettings::load_yaml(&p).ok())
        .unwrap_or_default();
    let state_dir = resolve_state_dir();
    let cfg_path = resolve_config_path(&state_dir);
    let config = match AppConfig::load_or_default(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            if strict_config {
                fail_json("config_error", &e);
            }
            AppConfig::default()
        }
    };
    let driver: Box<dyn OsDriver> = if use_mock {
        Box::new(MockDriver::new())
    } else {
        default_driver()
    };
    Gateway::with_config(driver, settings, state_dir, config)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DoctorCliAction {
    Help(&'static str),
    Run {
        onboard: bool,
        force: bool,
        auto_close: bool,
        check: bool,
        use_mock: bool,
    },
}

fn parse_doctor_cli_args(
    rest: &[String],
    mut use_mock: bool,
    default_onboard: bool,
) -> Result<DoctorCliAction, String> {
    let mut onboard = default_onboard;
    let mut force = false;
    let mut auto_close = true;
    let mut check = false;
    let cmd_name = if default_onboard { "onboard" } else { "doctor" };
    let help_text = if default_onboard {
        "Usage: apexuse-mcp onboard [--force] [--no-auto-close] [--check] [--mock]"
    } else {
        "Usage: apexuse-mcp doctor [--onboard] [--force] [--no-auto-close] [--check] [--mock]"
    };

    for arg in rest {
        match arg.as_str() {
            "--onboard" if !default_onboard => onboard = true,
            "--force" => force = true,
            "--no-auto-close" => auto_close = false,
            "--check" => check = true,
            "--mock" => use_mock = true,
            "-h" | "--help" => return Ok(DoctorCliAction::Help(help_text)),
            other => return Err(format!("Unknown {cmd_name} argument: {other}")),
        }
    }

    if !onboard && (force || !auto_close) {
        return Err("--force and --no-auto-close require --onboard".to_string());
    }

    Ok(DoctorCliAction::Run {
        onboard,
        force,
        auto_close,
        check,
        use_mock,
    })
}

fn status_permissions_ready(doc: &JsonValue) -> bool {
    doc.get("ok").and_then(|v| v.as_bool()) == Some(true)
        && doc.get("accessibility").and_then(|v| v.as_bool()) == Some(true)
        && doc.get("screen_recording").and_then(|v| v.as_bool()) == Some(true)
}

fn run_doctor_or_onboard(rest: &[String], use_mock: bool, default_onboard: bool) {
    let action = match parse_doctor_cli_args(rest, use_mock, default_onboard) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            process::exit(2);
        }
    };
    match action {
        DoctorCliAction::Help(usage) => {
            println!("{usage}");
        }
        DoctorCliAction::Run {
            onboard,
            force,
            auto_close,
            check,
            use_mock,
        } => {
            let gw = build_gateway(use_mock, false);
            let out = if onboard {
                gw.onboard(force, auto_close)
            } else {
                gw.doctor()
            };
            println!("{}", out.to_json_string());
            if check && !status_permissions_ready(&out) {
                process::exit(1);
            }
        }
    }
}

fn run_doctor(rest: &[String], use_mock: bool) {
    run_doctor_or_onboard(rest, use_mock, false);
}

fn run_onboard(rest: &[String], use_mock: bool) {
    run_doctor_or_onboard(rest, use_mock, true);
}

fn split_flag_value<'a>(arg: &'a str, name: &str) -> Option<&'a str> {
    arg.strip_prefix(name).and_then(|s| s.strip_prefix('='))
}

fn parse_timeout_ms_value(raw: &str) -> Result<u64, String> {
    let ms: u64 = raw
        .trim()
        .parse()
        .map_err(|_| "invalid integer for --timeout-ms".to_string())?;
    if ms == 0 {
        return Err("--timeout-ms must be > 0".to_string());
    }
    Ok(ms)
}

fn parse_timeout_ms(raw: &str) -> u64 {
    parse_timeout_ms_value(raw).unwrap_or_else(|e| fail_json("invalid_argument", &e))
}

fn build_probe_cli_output(cmd: &str, args: &[String], probe_val: JsonValue) -> JsonValue {
    let mut map = match probe_val {
        JsonValue::Object(m) => m,
        _ => BTreeMap::new(),
    };
    map.insert("ok".to_string(), JsonValue::Bool(true));
    map.insert("command".to_string(), JsonValue::String(cmd.to_string()));
    map.insert(
        "args".to_string(),
        JsonValue::Array(args.iter().cloned().map(JsonValue::String).collect()),
    );
    JsonValue::Object(map)
}

fn attach_probe_result(
    val: &mut JsonValue,
    cmd: &str,
    args: &[String],
    probe_res: Result<JsonValue, String>,
) {
    let (probe_ok, probe_obj) = match probe_res {
        Ok(p) => (true, build_probe_cli_output(cmd, args, p)),
        Err(msg) => (
            false,
            JsonValue::object(vec![
                ("ok", JsonValue::Bool(false)),
                ("code", JsonValue::String("probe_failed".into())),
                ("message", JsonValue::String(msg)),
                ("command", JsonValue::String(cmd.to_string())),
                (
                    "args",
                    JsonValue::Array(args.iter().cloned().map(JsonValue::String).collect()),
                ),
            ]),
        ),
    };
    if let JsonValue::Object(ref mut map) = val {
        map.insert("probe".to_string(), probe_obj);
        if !probe_ok {
            map.insert("ok".to_string(), JsonValue::Bool(false));
        }
    }
}

fn run_probe(rest: &[String], mut probe_mock: bool) {
    let mut opts = InstallOptions::default();

    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if let Some(val) = split_flag_value(arg, "--policy") {
            if val.trim().is_empty() {
                fail_json("invalid_argument", "--policy requires a non-empty path");
            }
            opts.policy_path = Some(PathBuf::from(val));
        } else if let Some(val) = split_flag_value(arg, "--timeout-ms") {
            opts.probe_timeout_ms = parse_timeout_ms(val);
        } else {
            match arg {
                "--mock" => probe_mock = true,
                "--npx" => opts.use_npx = true,
                "--policy" => {
                    i += 1;
                    if i >= rest.len() || rest[i].trim().is_empty() {
                        fail_json("invalid_argument", "--policy requires a path");
                    }
                    opts.policy_path = Some(PathBuf::from(&rest[i]));
                }
                "--timeout-ms" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--timeout-ms requires an integer");
                    }
                    opts.probe_timeout_ms = parse_timeout_ms(&rest[i]);
                }
                "-h" | "--help" => {
                    println!(
                        "Usage: apexuse-mcp probe [--mock] [--npx] [--policy <path>] [--timeout-ms <n>]"
                    );
                    return;
                }
                other if other.starts_with('-') => {
                    fail_json("invalid_argument", &format!("Unknown probe flag: {other}"));
                }
                positional => {
                    fail_json(
                        "invalid_argument",
                        &format!("Unexpected positional argument for probe: {positional}"),
                    );
                }
            }
        }
        i += 1;
    }

    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("apexuse-mcp"));
    let (cmd, mut probe_args) = resolved_install_invocation("codex", &exe, &opts);
    if probe_mock {
        probe_args.push("--mock".to_string());
    }

    match probe_mcp_server(
        &cmd,
        &probe_args,
        opts.policy_path.as_deref(),
        Duration::from_millis(opts.probe_timeout_ms),
    ) {
        Ok(probe_val) => {
            let out = build_probe_cli_output(&cmd, &probe_args, probe_val);
            println!("{}", format_json_pretty(&out));
        }
        Err(err) => fail_json("probe_failed", &err),
    }
}

fn run_install(rest: &[String], mut probe_mock: bool) {
    let mut opts = InstallOptions::default();
    let mut agent: Option<String> = None;

    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if let Some(val) = split_flag_value(arg, "--scope") {
            opts.scope = parse_scope(val);
        } else if let Some(val) = split_flag_value(arg, "--policy") {
            if val.trim().is_empty() {
                fail_json("invalid_argument", "--policy requires a non-empty path");
            }
            opts.policy_path = Some(PathBuf::from(val));
        } else if let Some(val) = split_flag_value(arg, "--agent") {
            if agent.is_some() {
                fail_json("invalid_argument", "multiple agent arguments provided");
            }
            agent = Some(val.to_string());
        } else if let Some(val) = split_flag_value(arg, "--timeout-ms") {
            opts.probe_timeout_ms = parse_timeout_ms(val);
        } else {
            match arg {
                "--write" => opts.write = true,
                "--dry-run" => opts.dry_run = true,
                "--npx" => opts.use_npx = true,
                "--with-skill" => opts.with_skill = true,
                "--probe" => opts.probe = true,
                "--mock" => probe_mock = true,
                "--timeout-ms" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--timeout-ms requires an integer");
                    }
                    opts.probe_timeout_ms = parse_timeout_ms(&rest[i]);
                }
                "--scope" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--scope requires 'user' or 'project'");
                    }
                    opts.scope = parse_scope(&rest[i]);
                }
                "--policy" => {
                    i += 1;
                    if i >= rest.len() || rest[i].trim().is_empty() {
                        fail_json("invalid_argument", "--policy requires a path");
                    }
                    opts.policy_path = Some(PathBuf::from(&rest[i]));
                }
                "--agent" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--agent requires an agent name");
                    }
                    if agent.is_some() {
                        fail_json("invalid_argument", "multiple agent arguments provided");
                    }
                    agent = Some(rest[i].clone());
                }
                "-h" | "--help" => {
                    println!(
                        "Usage: apexuse-mcp install <agent|{}|all> [--write] [--dry-run] [--scope user|project] [--npx] [--policy <path>] [--with-skill] [--probe] [--timeout-ms <n>] [--mock]\nSupported targets: {}, {}, all",
                        CODEX_PLUGIN_TARGET,
                        SUPPORTED_AGENTS.join(", "),
                        CODEX_PLUGIN_TARGET
                    );
                    return;
                }
                other if other.starts_with('-') => {
                    fail_json("invalid_argument", &format!("Unknown install flag: {other}"));
                }
                positional => {
                    if agent.is_some() {
                        fail_json(
                            "invalid_argument",
                            &format!("Unexpected extra positional argument: {positional}"),
                        );
                    }
                    agent = Some(positional.to_string());
                }
            }
        }
        i += 1;
    }

    let Some(target_agent) = agent else {
        fail_json(
            "invalid_argument",
            &format!(
                "Missing agent target. Usage: apexuse-mcp install <agent|{}|all>. Supported: {}, {}, all",
                CODEX_PLUGIN_TARGET,
                SUPPORTED_AGENTS.join(", "),
                CODEX_PLUGIN_TARGET
            ),
        );
    };

    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("apexuse-mcp"));
    let res = if target_agent.eq_ignore_ascii_case("all") {
        install_all(&exe, &opts)
    } else {
        install_agent(&target_agent, &exe, &opts)
    };

    match res {
        Ok(mut val) => {
            if opts.probe {
                let probe_target = if target_agent.eq_ignore_ascii_case("all") {
                    "codex"
                } else {
                    target_agent.as_str()
                };
                let (cmd, mut probe_args) =
                    resolved_install_invocation(probe_target, &exe, &opts);
                if probe_mock {
                    probe_args.push("--mock".to_string());
                }
                let probe_res = probe_mcp_server(
                    &cmd,
                    &probe_args,
                    opts.policy_path.as_deref(),
                    Duration::from_millis(opts.probe_timeout_ms),
                );
                attach_probe_result(&mut val, &cmd, &probe_args, probe_res);
            }
            println!("{}", format_json_pretty(&val));
            if val.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                process::exit(1);
            }
        }
        Err(msg) => fail_json("install_error", &msg),
    }
}

fn parse_scope(raw: &str) -> InstallScope {
    match raw.trim().to_ascii_lowercase().as_str() {
        "user" => InstallScope::User,
        "project" => InstallScope::Project,
        other => fail_json(
            "invalid_argument",
            &format!("invalid scope '{other}', expected 'user' or 'project'"),
        ),
    }
}

fn run_config(rest: &[String]) {
    if rest.first().is_some_and(|a| a == "-h" || a == "--help") {
        println!("Usage: apexuse-mcp config [show | get <key> | set <key> <val> | reset]");
        return;
    }

    let state_dir = resolve_state_dir();
    let cfg_path = resolve_config_path(&state_dir);
    let sub = rest.first().map(|s| s.as_str()).unwrap_or("show");

    match sub {
        "show" => {
            if rest.len() > 1 {
                fail_json("invalid_argument", "config show accepts no extra arguments");
            }
            let cfg = match AppConfig::load_or_default(&cfg_path) {
                Ok(c) => c,
                Err(e) => fail_json("config_error", &e),
            };
            let out = JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                (
                    "path",
                    JsonValue::String(cfg_path.to_string_lossy().into_owned()),
                ),
                ("config", cfg.to_json()),
            ]);
            println!("{}", out.to_json_string());
        }
        "get" => {
            if rest.len() != 2 {
                fail_json("invalid_argument", "Usage: apexuse-mcp config get <key>");
            }
            let key = &rest[1];
            let cfg = match AppConfig::load_or_default(&cfg_path) {
                Ok(c) => c,
                Err(e) => fail_json("config_error", &e),
            };
            let val = match cfg.get_key(key) {
                Ok(v) => v,
                Err(e) => fail_json("config_error", &e),
            };
            let out = JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                (
                    "path",
                    JsonValue::String(cfg_path.to_string_lossy().into_owned()),
                ),
                ("key", JsonValue::String(key.clone())),
                ("value", val),
            ]);
            println!("{}", out.to_json_string());
        }
        "set" => {
            if rest.len() != 3 {
                fail_json("invalid_argument", "Usage: apexuse-mcp config set <key> <val>");
            }
            let key = &rest[1];
            let raw_val = &rest[2];
            let mut cfg = match AppConfig::load_or_default(&cfg_path) {
                Ok(c) => c,
                Err(e) => fail_json("config_error", &e),
            };
            if let Err(e) = cfg.set_key(key, raw_val) {
                fail_json("config_error", &e);
            }
            if let Err(e) = cfg.save(&cfg_path) {
                fail_json("config_error", &e);
            }
            let updated = cfg.get_key(key).unwrap_or(JsonValue::Null);
            let out = JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                (
                    "path",
                    JsonValue::String(cfg_path.to_string_lossy().into_owned()),
                ),
                ("key", JsonValue::String(key.clone())),
                ("value", updated),
                ("config", cfg.to_json()),
            ]);
            println!("{}", out.to_json_string());
        }
        "reset" => {
            if rest.len() > 1 {
                fail_json("invalid_argument", "config reset accepts no extra arguments");
            }
            let cfg = AppConfig::default();
            if let Err(e) = cfg.save(&cfg_path) {
                fail_json("config_error", &e);
            }
            let out = JsonValue::object(vec![
                ("ok", JsonValue::Bool(true)),
                (
                    "path",
                    JsonValue::String(cfg_path.to_string_lossy().into_owned()),
                ),
                ("reset", JsonValue::Bool(true)),
                ("config", cfg.to_json()),
            ]);
            println!("{}", out.to_json_string());
        }
        other => fail_json(
            "invalid_argument",
            &format!("Unknown config action '{other}'. Expected: show, get, set, reset"),
        ),
    }
}

fn save_b64_image(path: &Path, b64: &str) -> Result<(), String> {
    let bytes = base64_decode(b64)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        }
    }
    fs::write(path, &bytes)
        .map_err(|e| format!("failed to write image {}: {e}", path.display()))
}

fn parse_call_steps(raw: &str) -> Result<Vec<(String, JsonValue)>, String> {
    let parsed = parse_json(raw).map_err(|e| format!("invalid --calls JSON: {e}"))?;
    let items = parsed
        .as_array()
        .ok_or_else(|| "--calls payload must be a JSON array".to_string())?;
    let mut out = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("call step {idx} must be a JSON object"))?;
        let tool = obj
            .get("tool")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("call step {idx} missing non-empty string 'tool'"))?
            .to_string();
        let args = match obj.get("args") {
            None | Some(JsonValue::Null) => JsonValue::object(vec![]),
            Some(v) if v.as_object().is_some() => v.clone(),
            Some(_) => return Err(format!("call step {idx} 'args' must be a JSON object")),
        };
        out.push((tool, args));
    }
    Ok(out)
}

fn run_call(rest: &[String], mut use_mock: bool) {
    let mut tool_name: Option<String> = None;
    let mut args_raw: Option<String> = None;
    let mut calls_raw: Option<String> = None;
    let mut calls_file: Option<PathBuf> = None;
    let mut sleep_ms: u64 = 0;
    let mut save_image: Option<PathBuf> = None;

    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if let Some(val) = split_flag_value(arg, "--args") {
            args_raw = Some(val.to_string());
        } else if let Some(val) = split_flag_value(arg, "--calls") {
            calls_raw = Some(val.to_string());
        } else if let Some(val) = split_flag_value(arg, "--calls-file") {
            calls_file = Some(PathBuf::from(val));
        } else if let Some(val) = split_flag_value(arg, "--sleep-ms") {
            sleep_ms = val
                .parse()
                .unwrap_or_else(|_| fail_json("invalid_argument", "invalid integer for --sleep-ms"));
        } else if let Some(val) = split_flag_value(arg, "--save-image") {
            save_image = Some(PathBuf::from(val));
        } else {
            match arg {
                "--mock" => use_mock = true,
                "--args" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--args requires a JSON object string");
                    }
                    args_raw = Some(rest[i].clone());
                }
                "--calls" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--calls requires a JSON array string");
                    }
                    calls_raw = Some(rest[i].clone());
                }
                "--calls-file" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--calls-file requires a file path");
                    }
                    calls_file = Some(PathBuf::from(&rest[i]));
                }
                "--sleep-ms" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--sleep-ms requires an integer");
                    }
                    sleep_ms = rest[i].parse().unwrap_or_else(|_| {
                        fail_json("invalid_argument", "invalid integer for --sleep-ms")
                    });
                }
                "--save-image" => {
                    i += 1;
                    if i >= rest.len() {
                        fail_json("invalid_argument", "--save-image requires a file path");
                    }
                    save_image = Some(PathBuf::from(&rest[i]));
                }
                "-h" | "--help" => {
                    println!(
                        "Usage:\n\
                         \x20 apexuse-mcp call <tool> [--args '<json>'] [--save-image <path>] [--mock]\n\
                         \x20 apexuse-mcp call (--calls '<json-array>' | --calls-file <path>) [--sleep-ms <n>] [--save-image <path>] [--mock]"
                    );
                    return;
                }
                other if other.starts_with('-') => {
                    fail_json("invalid_argument", &format!("Unknown call flag: {other}"));
                }
                positional => {
                    if tool_name.is_some() {
                        fail_json(
                            "invalid_argument",
                            &format!("Unexpected extra positional argument: {positional}"),
                        );
                    }
                    tool_name = Some(positional.to_string());
                }
            }
        }
        i += 1;
    }

    if calls_raw.is_some() && calls_file.is_some() {
        fail_json(
            "invalid_argument",
            "cannot pass both --calls and --calls-file",
        );
    }
    let has_batch = calls_raw.is_some() || calls_file.is_some();
    if has_batch && (tool_name.is_some() || args_raw.is_some()) {
        fail_json(
            "invalid_argument",
            "cannot combine <tool>/--args with --calls/--calls-file",
        );
    }

    if let Some(tool) = tool_name {
        let args = match args_raw {
            None => JsonValue::object(vec![]),
            Some(raw) => {
                let parsed = parse_json(&raw)
                    .unwrap_or_else(|e| fail_json("invalid_argument", &format!("invalid --args JSON: {e}")));
                if parsed.as_object().is_none() {
                    fail_json("invalid_argument", "--args must be a JSON object");
                }
                parsed
            }
        };
        let mut gw = build_gateway(use_mock, true);
        let out = gw.dispatch(&tool, &args);
        if let (Some(img_path), Some(b64)) = (&save_image, &out.image_b64) {
            if let Err(e) = save_b64_image(img_path, b64) {
                fail_json("image_save_error", &e);
            }
        }
        let ok = out.payload.get("ok").and_then(|v| v.as_bool()) == Some(true);
        println!("{}", out.payload.to_json_string());
        if !ok {
            process::exit(1);
        }
        return;
    }

    if has_batch {
        let batch_json = if let Some(raw) = calls_raw {
            raw
        } else if let Some(path) = calls_file {
            fs::read_to_string(&path).unwrap_or_else(|e| {
                fail_json(
                    "invalid_argument",
                    &format!("failed to read --calls-file {}: {e}", path.display()),
                )
            })
        } else {
            unreachable!()
        };

        let steps = parse_call_steps(&batch_json)
            .unwrap_or_else(|e| fail_json("invalid_argument", &e));
        let total = steps.len();
        let mut gw = build_gateway(use_mock, true);
        let mut results = Vec::with_capacity(total);
        let mut all_ok = true;
        let mut failed_index = None;

        for (idx, (tool, step_args)) in steps.into_iter().enumerate() {
            if idx > 0 && sleep_ms > 0 {
                std::thread::sleep(Duration::from_millis(sleep_ms));
            }
            let out = gw.dispatch(&tool, &step_args);
            if let (Some(img_path), Some(b64)) = (&save_image, &out.image_b64) {
                if let Err(e) = save_b64_image(img_path, b64) {
                    fail_json("image_save_error", &e);
                }
            }
            let step_ok = out.payload.get("ok").and_then(|v| v.as_bool()) == Some(true);
            results.push(JsonValue::object(vec![
                ("index", JsonValue::Number(idx as f64)),
                ("tool", JsonValue::String(tool)),
                ("ok", JsonValue::Bool(step_ok)),
                ("result", out.payload),
            ]));
            if !step_ok {
                all_ok = false;
                failed_index = Some(idx);
                break;
            }
        }

        let mut fields = vec![
            ("ok", JsonValue::Bool(all_ok)),
            ("total", JsonValue::Number(total as f64)),
            ("completed", JsonValue::Number(results.len() as f64)),
            ("results", JsonValue::Array(results)),
        ];
        if let Some(fi) = failed_index {
            fields.push(("failed_index", JsonValue::Number(fi as f64)));
        }
        println!("{}", JsonValue::object(fields).to_json_string());
        if !all_ok {
            process::exit(1);
        }
        return;
    }

    fail_json(
        "invalid_argument",
        "Missing <tool> or --calls / --calls-file. Run `apexuse-mcp call --help`.",
    );
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut use_mock = first_non_empty_env(&["APEXUSE_MOCK", "HANDSMCP_MOCK"])
        .map(|v| v == "1")
        .unwrap_or(false);

    let mut pos = 1;
    let mut cli_mock = false;
    while pos < args.len() && args[pos] == "--mock" {
        use_mock = true;
        cli_mock = true;
        pos += 1;
    }

    if pos >= args.len() {
        let mut gw = build_gateway(use_mock, true);
        if let Err(e) = run_stdio_loop(&mut gw) {
            eprintln!("MCP stdio error: {e}");
            process::exit(1);
        }
        return;
    }

    match args[pos].as_str() {
        "--version" | "-V" | "version" => {
            println!("apexuse-mcp {}", env!("CARGO_PKG_VERSION"));
        }
        "--help" | "-h" | "help" => {
            print_usage();
        }
        "doctor" | "--doctor" => {
            run_doctor(&args[pos + 1..], use_mock);
        }
        "onboard" | "--onboard" => {
            run_onboard(&args[pos + 1..], use_mock);
        }
        "probe" => {
            run_probe(&args[pos + 1..], cli_mock);
        }
        "install" | "--install" => {
            run_install(&args[pos + 1..], cli_mock);
        }
        "config" => {
            run_config(&args[pos + 1..]);
        }
        "call" => {
            run_call(&args[pos + 1..], use_mock);
        }
        other => {
            eprintln!("Unknown argument: {other}");
            process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_timeout_ms_value_validates_positive_integers() {
        assert_eq!(parse_timeout_ms_value("10000"), Ok(10_000));
        assert_eq!(parse_timeout_ms_value(" 250 "), Ok(250));
        assert!(parse_timeout_ms_value("0").is_err());
        assert!(parse_timeout_ms_value("-10").is_err());
        assert!(parse_timeout_ms_value("abc").is_err());
    }

    #[test]
    fn attach_probe_result_handles_success_failure_and_existing_install_failure() {
        let args = vec!["--mock".to_string()];
        let probe_ok = JsonValue::object(vec![
            ("ok", JsonValue::Bool(true)),
            ("server_name", JsonValue::String("apexuse-mcp".into())),
            ("server_version", JsonValue::String("0.3.0".into())),
            ("tools_count", JsonValue::Number(6.0)),
            ("tools", JsonValue::Array(vec![JsonValue::String("inspect_app".into())])),
        ]);

        // Success preserves ok: true and attaches probe metadata
        let mut ok_install = JsonValue::object(vec![("ok", JsonValue::Bool(true))]);
        attach_probe_result(&mut ok_install, "/bin/apexuse-mcp", &args, Ok(probe_ok.clone()));
        assert_eq!(ok_install.get("ok").and_then(|v| v.as_bool()), Some(true));
        let p = ok_install.get("probe").unwrap();
        assert_eq!(p.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            p.get("command").and_then(|v| v.as_str()),
            Some("/bin/apexuse-mcp")
        );
        assert_eq!(p.get("tools_count").and_then(|v| v.as_i64()), Some(6));

        // Success does not flip already-false install_all ok: false to true
        let mut partial_fail = JsonValue::object(vec![("ok", JsonValue::Bool(false))]);
        attach_probe_result(&mut partial_fail, "/bin/apexuse-mcp", &args, Ok(probe_ok));
        assert_eq!(partial_fail.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            partial_fail
                .get("probe")
                .and_then(|v| v.get("ok"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        // Probe failure flips top-level ok to false and sets probe.code = "probe_failed"
        let mut failed_probe = JsonValue::object(vec![("ok", JsonValue::Bool(true))]);
        attach_probe_result(
            &mut failed_probe,
            "npx",
            &["-y".into(), "apexuse-mcp".into()],
            Err("timed out".into()),
        );
        assert_eq!(failed_probe.get("ok").and_then(|v| v.as_bool()), Some(false));
        let fp = failed_probe.get("probe").unwrap();
        assert_eq!(fp.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(fp.get("code").and_then(|v| v.as_str()), Some("probe_failed"));
        assert_eq!(fp.get("message").and_then(|v| v.as_str()), Some("timed out"));
    }

    #[test]
    fn parse_doctor_and_onboard_cli_args_handles_flags() {
        let plain = parse_doctor_cli_args(&[], false, false).unwrap();
        assert_eq!(
            plain,
            DoctorCliAction::Run {
                onboard: false,
                force: false,
                auto_close: true,
                check: false,
                use_mock: false,
            }
        );

        let doc_onboard = parse_doctor_cli_args(
            &[
                "--onboard".into(),
                "--force".into(),
                "--no-auto-close".into(),
                "--check".into(),
                "--mock".into(),
            ],
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            doc_onboard,
            DoctorCliAction::Run {
                onboard: true,
                force: true,
                auto_close: false,
                check: true,
                use_mock: true,
            }
        );

        let ob = parse_doctor_cli_args(
            &["--no-auto-close".into(), "--check".into()],
            true,
            true,
        )
        .unwrap();
        assert_eq!(
            ob,
            DoctorCliAction::Run {
                onboard: true,
                force: false,
                auto_close: false,
                check: true,
                use_mock: true,
            }
        );

        // --force without --onboard on doctor is rejected cleanly
        assert!(parse_doctor_cli_args(&["--force".into()], false, false).is_err());
        assert!(parse_doctor_cli_args(&["--bogus".into()], false, true).is_err());
    }

    #[test]
    fn status_permissions_ready_checks_ok_ax_and_sr() {
        let ok = JsonValue::object(vec![
            ("ok", JsonValue::Bool(true)),
            ("accessibility", JsonValue::Bool(true)),
            ("screen_recording", JsonValue::Bool(true)),
        ]);
        assert!(status_permissions_ready(&ok));

        let missing_sr = JsonValue::object(vec![
            ("ok", JsonValue::Bool(true)),
            ("accessibility", JsonValue::Bool(true)),
            ("screen_recording", JsonValue::Bool(false)),
        ]);
        assert!(!status_permissions_ready(&missing_sr));

        let err = JsonValue::object(vec![
            ("ok", JsonValue::Bool(false)),
            ("accessibility", JsonValue::Bool(true)),
            ("screen_recording", JsonValue::Bool(true)),
        ]);
        assert!(!status_permissions_ready(&err));
    }
}
