use apexuse_mcp::driver::{default_driver, OsDriver};
use apexuse_mcp::gateway::{Gateway, MockDriver};
use apexuse_mcp::json::JsonValue;
use apexuse_mcp::mcp::run_stdio_loop;
use apexuse_mcp::policy::PolicySettings;
use std::env;
use std::path::{Path, PathBuf};
use std::process;

fn first_non_empty_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| env::var(k).ok().filter(|v| !v.is_empty()))
}

fn resolve_state_dir() -> PathBuf {
    if let Some(custom) = first_non_empty_env(&["APEXUSE_STATE_DIR", "HANDSMCP_STATE_DIR"]) {
        return PathBuf::from(custom);
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(".apexuse");
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

fn print_install_config(agent: &str, exe_path: &Path) -> Result<String, String> {
    let bin = exe_path.display().to_string();
    match agent {
        "antigravity" | "gemini" => {
            let cfg = JsonValue::object(vec![(
                "mcpServers",
                JsonValue::object(vec![(
                    "apexuse",
                    JsonValue::object(vec![
                        ("command", JsonValue::String(bin)),
                        ("args", JsonValue::Array(vec![])),
                    ]),
                )]),
            )]);
            Ok(cfg.to_json_string())
        }
        "codex" => Ok(format!(
            "[mcp_servers.apexuse]\ncommand = {}\nargs = []\nstartup_timeout_sec = 60\ntool_timeout_sec = 180",
            JsonValue::String(bin).to_json_string()
        )),
        "claude" => Ok(format!("claude mcp add apexuse -- {bin}")),
        other => Err(format!("Unsupported agent: {other}")),
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut use_mock = first_non_empty_env(&["APEXUSE_MOCK", "HANDSMCP_MOCK"])
        .map(|v| v == "1")
        .unwrap_or(false);
    let mut cmd = "mcp";
    let mut install_agent: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--mock" => use_mock = true,
            "--doctor" | "doctor" => cmd = "doctor",
            "--install" | "install" => {
                cmd = "install";
                if i + 1 < args.len() {
                    i += 1;
                    install_agent = Some(args[i].clone());
                }
            }
            "--agent" if i + 1 < args.len() => {
                i += 1;
                install_agent = Some(args[i].clone());
            }
            "--help" | "-h" => {
                println!(
                    "apexuse-mcp — Cross-platform Hybrid Accessibility + Set-of-Mark Visual Indexing MCP\n\
                     Usage:\n\
                     \x20 apexuse-mcp [--mock]                 Start MCP stdio server\n\
                     \x20 apexuse-mcp doctor                   Print OS accessibility & screen capture readiness\n\
                     \x20 apexuse-mcp install <agent>          Print MCP config (antigravity|gemini|claude|codex)"
                );
                return;
            }
            other => {
                eprintln!("Unknown argument: {other}");
                process::exit(2);
            }
        }
        i += 1;
    }

    if cmd == "install" {
        let agent = install_agent.unwrap_or_else(|| "antigravity".into());
        let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("apexuse-mcp"));
        match print_install_config(&agent, &exe) {
            Ok(out) => {
                println!("{out}");
                return;
            }
            Err(e) => {
                eprintln!("{e}");
                process::exit(2);
            }
        }
    }

    let settings = resolve_policy_path()
        .and_then(|p| PolicySettings::load_yaml(&p).ok())
        .unwrap_or_default();
    let state_dir = resolve_state_dir();
    let driver: Box<dyn OsDriver> = if use_mock {
        Box::new(MockDriver::new())
    } else {
        default_driver()
    };
    let mut gw = Gateway::new(driver, settings, state_dir);

    if cmd == "doctor" {
        println!("{}", gw.doctor().to_json_string());
        return;
    }

    if let Err(e) = run_stdio_loop(&mut gw) {
        eprintln!("MCP stdio error: {e}");
        process::exit(1);
    }
}
