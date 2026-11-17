# ApexUse v0.3.0 UX Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Codex Desktop Plugin (`install codex-plugin`), pure-Rust post-install MCP probe (`--probe` / `probe`), host-aware macOS AppKit permission onboarding (`onboard` / `doctor --onboard`), `npm` `postinstall` guidance, and rich `npx skills add` metadata on `feat/v0.3.1-codex-plugin-onboarding`.

**Architecture:** Preserve `apexuse-mcp` as a single dependency-free cross-platform Rust binary + thin npm wrapper while integrating new UX modules cleanly: `installer.rs` + embedded `rust/assets/plugin/` manage Codex plugin caching/marketplace registration and mutual conflict-free switching; `probe.rs` performs bounded JSON-RPC stdio handshakes via `std::process::Command`; `macos_bridge.{h,m}` + `driver/macos.rs` present an on-demand `.accessory` AppKit onboarding window identifying the host app and polling Accessibility/Screen Recording via short-lived child checks; `npm/apexuse-mcp/scripts/postinstall.js` and `.agents/skills/apexuse/{agents/openai.yaml,references/*}` complete onboarding and skill discovery.

**Tech Stack:** Rust (`std` only, edition 2021), Objective-C (AppKit / ApplicationServices / CoreGraphics / Foundation via `cc`/`clang` in `rust/build.rs`), Node.js (`>=18`, built-in modules only), Python 3.11 + `pytest` + `mcp` E2E tests.

**Spec:** [docs/superpowers/specs/2026-11-17-onboarding-codex-plugin-ux-design.md](file:///Users/pawankumar/handsmcp/docs/superpowers/specs/2026-11-17-onboarding-codex-plugin-ux-design.md)

## Global Constraints

- `rust/Cargo.toml`: MUST retain **0 external Rust crates**.
- `npm/apexuse-mcp/package.json`: MUST retain `"dependencies": {}`.
- Target release version across all manifests (`Cargo.toml`, `npm/apexuse-mcp/package.json`, `pyproject.toml`, `server.json`, `plugins/apexuse/.codex-plugin/plugin.json`, and `mcp.rs` `serverInfo.version`) MUST stay unified at `0.3.0` (`env!("CARGO_PKG_VERSION")` in `mcp.rs`).
- Compiler strictness: 0 warnings under `xcrun clang -fobjc-arc -O2 -mmacosx-version-min=14.0 -Wall -Wextra -Werror -fsyntax-only rust/native/macos_bridge.m`, `cargo check --all-targets`, `--target x86_64-pc-windows-gnu --lib`, and `--target x86_64-unknown-linux-gnu --lib`.
- Backward compatibility: zero-argument `apexuse-mcp` MUST still run `mcp::run_stdio_loop`, and `apexuse-mcp doctor --check` MUST keep its exact JSON contract without spawning any GUI window.
- Crate asset sync: `rust/assets/SKILL.md` and `rust/assets/plugin/*` MUST live inside `rust/` and match repo files cleanly (`cargo publish --dry-run` verifies tarball compilation).

## Review Focus

- Quoted Codex TOML headers (`[plugins."apexuse@apexuse-local"]`, `[marketplaces.apexuse-local]`, `[mcp_servers."apexuse"]`) must parse, update, preserve sibling sections/multiline strings, and never create duplicate `"apexuse"` registrations when switching `codex` <-> `codex-plugin`.
- Windows & POSIX backslash escaping in Codex `config.toml` `source = "..."` and `.mcp.json` commands/policies must emit valid JSON and valid TOML basic strings.
- `probe_mcp_server` must always kill and wait/reap spawned child processes on success, invalid JSON/non-matching `serverInfo.name`, stdout EOF, or timeout without deadlocking on noisy stderr.
- `install_all_with_env` must inspect existing `~/.codex/config.toml` first and refresh active `codex-plugin` in place when `[plugins."apexuse@apexuse-local"]` has `enabled = true` rather than overwriting Codex back to `[mcp_servers.apexuse]`.
- `apexuse_macos_onboard` must fast-return without UI when `!force && ax && sr`, spawn short-lived `doctor` child checks for live Screen Recording polling (with tight child timeout so main UI thread never freezes), and never open UI during headless/CLI `--mock` tests.

---

### Task 1: Codex Desktop Plugin Assets, Crate Bundle & `install codex-plugin`

**Files:**
- Create: `.agents/plugins/marketplace.json`
- Create: `plugins/apexuse/.codex-plugin/plugin.json`
- Create: `plugins/apexuse/.mcp.json`
- Create: `plugins/apexuse/assets/apexuse.svg`
- Create: `plugins/apexuse/assets/apexuse-small.svg`
- Create: `rust/assets/plugin/marketplace.json`
- Create: `rust/assets/plugin/plugin.json`
- Create: `rust/assets/plugin/apexuse.svg`
- Create: `rust/assets/plugin/apexuse-small.svg`
- Modify: `rust/src/mcp.rs:400-406`
- Modify: `rust/src/installer.rs:1-160, 429-704, 811-1140`
- Modify: `rust/src/main.rs:72-98, 156-255`

**Interfaces:**
- Produces:
  - `pub const CODEX_PLUGIN_TARGET: &str = "codex-plugin";`
  - `pub(crate) fn resolved_install_invocation(raw_target: &str, exe_path: &Path, opts: &InstallOptions) -> (String, Vec<String>)` in `rust/src/installer.rs` (consumed by Task 2 for `--probe`).
  - `rust/src/mcp.rs` `initialize` `serverInfo.version = env!("CARGO_PKG_VERSION")`.

- [ ] **Step 1: Write failing unit tests for `codex-plugin` and `mcp.rs` dynamic version**

Add tests in `rust/src/installer.rs`:
- `codex_plugin_upserts_marketplace_materializes_cache_and_clears_mcp_server`: invokes `install_agent_with_env("codex-plugin", Path::new("/opt/apexuse-mcp"), &opts_write, &env)` on a pre-existing `config.toml` containing `model = "gpt-5"`, `[mcp_servers.context7]`, and `[mcp_servers.apexuse]`; asserts `ok == true`, `config.toml` contains `[marketplaces.apexuse-local]`, `source_type = "local"`, `[plugins."apexuse@apexuse-local"]`, `enabled = true`, keeps `[mcp_servers.context7]`, removes `[mcp_servers.apexuse]`/`[mcp_servers.apexuse.env]`, writes `.bak`, writes `~/.codex/plugins/cache/apexuse-local/apexuse/0.3.0/{.codex-plugin/plugin.json,.mcp.json,assets/apexuse.svg,assets/apexuse-small.svg}` and `~/.codex/plugins/marketplaces/apexuse-local/.agents/plugins/marketplace.json`, and asserts re-running returns `status == "unchanged"`.
- `codex_plugin_rejects_project_scope_and_install_all_preserves_active_plugin`: asserts `--scope project` on `codex-plugin` fails; asserts `install_all_with_env` when `[plugins."apexuse@apexuse-local"]` has `enabled = true` preserves/refreshes `codex-plugin` (does not re-add `[mcp_servers.apexuse]`); asserts explicit `install_agent_with_env("codex", ...)` disables `[plugins."apexuse@apexuse-local"]` (`enabled = false`) and writes `[mcp_servers.apexuse]`.
- `codex_plugin_escapes_windows_paths_in_toml_and_mcp_json`: exercises `TargetOs::Windows` with backslash `home = C:\Users\alice` and verifies `config.toml` and cached `.mcp.json` parse/roundtrip cleanly.

In `rust/src/mcp.rs` (`mcp_protocol_initialize_list_and_visual_call`), assert `serverInfo.version == env!("CARGO_PKG_VERSION")`.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path rust/Cargo.toml installer::tests::codex_plugin`
Expected: FAIL because `codex-plugin` target does not exist yet.

- [ ] **Step 3: Create plugin assets, mirror them under `rust/assets/plugin/`, and implement `codex-plugin` + `mcp.rs` version env**

1. Create `.agents/plugins/marketplace.json`, `plugins/apexuse/.codex-plugin/plugin.json`, `plugins/apexuse/.mcp.json`, `plugins/apexuse/assets/apexuse.svg`, and `plugins/apexuse/assets/apexuse-small.svg` matching Spec §4.A.1, and copy the mirrored template files into `rust/assets/plugin/{marketplace.json,plugin.json,apexuse.svg,apexuse-small.svg}`.
2. Update `rust/src/mcp.rs:403` to `env!("CARGO_PKG_VERSION")`.
3. In `rust/src/installer.rs`:
   - Embed `rust/assets/plugin/*` via `include_str!` cleanly (`PLUGIN_MARKETPLACE_JSON`, `PLUGIN_MANIFEST_JSON`, `PLUGIN_ICON_SVG`, `PLUGIN_SMALL_ICON_SVG`).
   - Refactor TOML section replacement into parameterized section predicates/headers so `[mcp_servers.apexuse*]`, `[marketplaces.apexuse-local]`, and `[plugins."apexuse@apexuse-local"]` share clean single-pass section rewriting (no duplicated TOML parsers).
   - Implement `codex-plugin` materialization (`~/.codex/plugins/marketplaces/apexuse-local/` + `~/.codex/plugins/cache/apexuse-local/apexuse/<ver>/` with dynamic `.mcp.json` rendered from `(cmd, args, opts.policy_path)`), mutual conflict-free switching (`codex` <-> `codex-plugin`), active-plugin preservation in `install_all_with_env`, and export `resolved_install_invocation`.
   - Expose `install codex-plugin` in `rust/src/main.rs` help/usage without breaking `SUPPORTED_AGENTS` 9-agent `install all` defaults.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path rust/Cargo.toml`
Expected: PASS (0 warnings across host, Windows, and Linux `cargo check`).

- [ ] **Step 5: Commit**

```bash
git add .agents/plugins plugins/apexuse rust/assets/plugin rust/src/installer.rs rust/src/mcp.rs rust/src/main.rs
git commit -m "feat(codex-plugin): bundle Codex desktop plugin assets and installer support"
```

---

### Task 2: Live Post-Install MCP Handshake Probe (`probe.rs`, `--probe`, `apexuse-mcp probe`)

**Files:**
- Create: `rust/src/probe.rs`
- Modify: `rust/src/lib.rs`
- Modify: `rust/src/installer.rs`
- Modify: `rust/src/main.rs:72-98, 156-255, 632-655`

**Interfaces:**
- Consumes: `installer::resolved_install_invocation` from Task 1.
- Produces:
  - `pub fn probe_mcp_server(cmd: &str, args: &[String], policy_path: Option<&Path>, timeout: Duration) -> Result<JsonValue, String>` in `rust/src/probe.rs`.
  - `pub struct InstallOptions { ..., pub probe: bool, pub probe_timeout_ms: u64 }` in `rust/src/installer.rs` (default `probe: false`, `probe_timeout_ms: 10_000`).

- [ ] **Step 1: Write failing unit tests in `rust/src/probe.rs` and `rust/src/installer.rs`**

Add unit tests covering:
- `probe_validates_initialize_and_tools_list`: spawns a small OS shell/helper stdio script (Python `-c` or `apexuse-mcp` test fixture using standard `Command`) returning valid `initialize` (`serverInfo: { name: "apexuse-mcp", version: "0.3.0" }`) + 6 unique tools, asserting `ok == true`, `tools_count == 6`, and `server_name == "apexuse-mcp"`. *(Review Focus: use `std::env::current_exe()` or Python `-c`? Note unit tests run inside `cargo test` test binary, so `sh -c` / `python3 -c` or `apexuse-mcp --mock`? Using `python3 -c`? Wait — CI Windows runners may lack `sh` or `python3` in rust-only job order! Better: inside `probe.rs`, factor `probe_io_streams<R: BufRead, W: Write>(reader: R, writer: W, timeout: Duration)` OR spawn `std::env::current_exe()` with a `#[test]` subprocess helper env flag? Actually checking BOTH: pure-stream `probe_reader_writer` unit tests run deterministically on ALL OSes with zero shell deps, AND `tests/test_rust_mcp.py` tests `apexuse-mcp probe --mock` + `install ... --probe` end-to-end against the real binary! Plus a real `Command` timeout test on a harmless sleeper / immediate-EOF subprocess!)*
- `probe_rejects_wrong_server_name_malformed_json_and_timeout`: tests wrong `serverInfo.name`, non-JSON stdout, premature EOF, and timeout expiry reaping cleanly.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path rust/Cargo.toml probe::tests`
Expected: FAIL (`probe` module missing).

- [ ] **Step 3: Implement `rust/src/probe.rs` and wire `--probe` + `probe` CLI subcommand**

1. Create `rust/src/probe.rs`:
   - `probe_mcp_server(cmd: &str, args: &[String], policy_path: Option<&Path>, timeout: Duration) -> Result<JsonValue, String>`:
     - Spawns `Command::new(cmd).args(args)` with optional `APEXUSE_POLICY`, `stdin(Stdio::piped())`, `stdout(Stdio::piped())`, `stderr(Stdio::piped())`.
     - Reads stdout lines in a bounded background thread (`mpsc::channel`), drains/caps stderr (`4 KiB`) in another thread so noisy stderr never blocks child pipe buffers!
     - Runs JSON-RPC `initialize`, `notifications/initialized`, and `tools/list` under a single shared deadline.
     - Always calls `let _ = child.kill(); let _ = child.wait();` cleanly in a drop guard / finally path.
2. Export `pub mod probe;` in `rust/src/lib.rs`.
3. Update `rust/src/main.rs`:
   - Add `--probe` and `--timeout-ms <n>` flags on `install` (`--mock` allowed with `--probe` when invoked in offline tests so `resolved_invocation` can append `--mock` only if explicitly requested!). Attach `"probe": probe_val` to install JSON output; exit `1` if `probe` fails.
   - Add top-level `apexuse-mcp probe [--mock] [--npx] [--policy <path>] [--timeout-ms <n>]` command printing structured JSON and returning exit `0` / `1`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path rust/Cargo.toml`
Expected: PASS (0 warnings on host, Windows, and Linux `cargo check`).

- [ ] **Step 5: Commit**

```bash
git add rust/src/probe.rs rust/src/lib.rs rust/src/installer.rs rust/src/main.rs
git commit -m "feat(probe): add pure-Rust live MCP handshake probe command and install --probe"
```

---

### Task 3: Host-Aware macOS AppKit Permission Onboarding UI (`onboard` / `doctor --onboard`)

**Files:**
- Modify: `rust/native/macos_bridge.h:1-26`
- Modify: `rust/native/macos_bridge.m:829-842` (+ new AppKit onboarding window section)
- Modify: `rust/src/driver/mod.rs:19-26, 598-655`
- Modify: `rust/src/driver/macos.rs:18-60, 105-125`
- Modify: `rust/src/gateway.rs:237-260` (+ `MockDriver` `onboard` support + tests)
- Modify: `rust/src/main.rs:72-98, 123-151, 632-655`

**Interfaces:**
- Produces:
  - `char *apexuse_macos_onboard(int force, int auto_close);` in `macos_bridge.{h,m}` (`force != 0` opens window even when already granted).
  - `pub struct OnboardStatus` and `OsDriver::onboard(&self, force: bool, auto_close: bool) -> Result<OnboardStatus, DriverError>` in `rust/src/driver/mod.rs` (default impl for Linux/Windows returning `shown: false`).
  - `Gateway::onboard(&self, force: bool, auto_close: bool) -> JsonValue` and `Gateway::doctor()` adding `"onboard_hint"` only on macOS when `!accessibility || !screen_recording`.
  - CLI subcommands: `apexuse-mcp onboard [--force] [--no-auto-close] [--check] [--mock]` and `apexuse-mcp doctor --onboard [--force] [--no-auto-close] [--check] [--mock]`.

- [ ] **Step 1: Write failing unit tests in `gateway.rs` and `driver::macos::tests`**

Add unit tests covering:
- `doctor_includes_onboard_hint_only_when_macos_missing_permissions`: tests `MockDriver` with `os="macos", accessibility=false` emits `"onboard_hint"`, while `accessibility=true, screen_recording=true` (and `os="linux"`) omits `"onboard_hint"`.
- `gateway_onboard_handles_mock_granted_and_missing_states`: tests `Gateway::onboard(false, true)` returns `{ ok: true, shown: false, accessibility, screen_recording }`.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path rust/Cargo.toml onboard`
Expected: FAIL (`onboard` method/struct not defined yet).

- [ ] **Step 3: Implement `macos_bridge.{h,m}` AppKit onboarding & wire `driver`/`gateway`/`main`**

1. In `rust/native/macos_bridge.h` and `rust/native/macos_bridge.m`:
   - Add helper detecting host/responsible application name by walking ancestor PIDs (`getpid()` -> `getppid()` / `sysctl` `CTL_KERN, KERN_PROC, KERN_PROC_PID`) with `[NSRunningApplication runningApplicationWithProcessIdentifier:...]` preferring the nearest ancestor with `activationPolicy == NSApplicationActivationPolicyRegular`, falling back to `[[NSRunningApplication currentApplication] localizedName]` / executable basename.
   - Add helper checking Screen Recording via short-lived child `doctor` subprocess (`NSTask` pointing to `NSBundle.mainBundle.executablePath` with `--doctor` / `doctor`, bounded `1.5s` timeout so `NSTimer` never blocks main thread!) fallback cleanly to `CGPreflightScreenCaptureAccess()`.
   - Implement `apexuse_macos_onboard(int force, int auto_close)`:
     - Fast-path when `!force && AXIsProcessTrusted() && CGPreflightScreenCaptureAccess()` returns `{ ok: true, os: "macos", shown: false, already_granted: true, accessibility: true, screen_recording: true }` immediately without touching `NSApplication` activation policy or creating windows.
     - GUI path creates a clean `NSWindow` (`"ApexUse — macOS Permissions"`), promotes `[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory]`, shows detected host app instructions, live Accessibility/Screen Recording status cards, **Grant Accessibility** (`AXIsProcessTrustedWithOptions` prompt + `Privacy_Accessibility` URL), **Grant Screen Recording** (`CGRequestScreenCaptureAccess` + `Privacy_ScreenCapture` URL), **Re-check**, **Done**, host-restart note when Screen Recording still reflects `false`, `0.75s` `NSTimer` refresh, and clean modal loop teardown (`stopModal` + restoring activation policy).
2. Wire `OnboardStatus` / `OsDriver::onboard` in `driver/mod.rs`, `driver/macos.rs`, `gateway.rs`, and `main.rs` (`onboard` and `doctor --onboard`, keeping `doctor --check` strict and UI-free unless `--onboard` is explicitly given).

- [ ] **Step 4: Run native clang strict check, cross-platform cargo checks, and tests**

Run:
- `xcrun clang -fobjc-arc -O2 -mmacosx-version-min=14.0 -Wall -Wextra -Werror -fsyntax-only rust/native/macos_bridge.m`
- `cargo check --manifest-path rust/Cargo.toml --all-targets`
- `cargo check --manifest-path rust/Cargo.toml --target x86_64-pc-windows-gnu --lib`
- `cargo check --manifest-path rust/Cargo.toml --target x86_64-unknown-linux-gnu --lib`
- `cargo test --manifest-path rust/Cargo.toml`
Expected: 0 warnings across all checks; all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/native/macos_bridge.h rust/native/macos_bridge.m rust/src/driver/mod.rs rust/src/driver/macos.rs rust/src/gateway.rs rust/src/main.rs
git commit -m "feat(macos,cli): add host-aware AppKit permission onboarding window and onboard CLI"
```

---

### Task 4: `npm postinstall` Banner, Rich `npx skills add` Pack, Docs & Cross-Platform E2E

**Files:**
- Create: `npm/apexuse-mcp/scripts/postinstall.js`
- Modify: `npm/apexuse-mcp/package.json`
- Modify: `npm/apexuse-mcp/test/launcher.test.js`
- Create: `.agents/skills/apexuse/agents/openai.yaml`
- Create: `.agents/skills/apexuse/references/installation.md`
- Create: `.agents/skills/apexuse/references/usage.md`
- Create: `.agents/skills/apexuse/references/troubleshooting.md`
- Modify: `README.md`, `agent-configs/README.md`, `llms.txt`, `npm/apexuse-mcp/README.md`
- Modify: `tests/test_rust_mcp.py`

**Interfaces:**
- Consumes: Tasks 1–3 CLI commands (`install codex-plugin`, `--probe`, `probe`, `onboard`, `doctor --onboard`).

- [ ] **Step 1: Add failing Node `postinstall.js` unit tests & Python E2E assertions**

1. In `npm/apexuse-mcp/test/launcher.test.js`, import `renderPostinstallMessage` from `../scripts/postinstall.js` and test macOS (`doctor --onboard`), non-macOS (`doctor --check`), `install all --write --npx --with-skill --probe`, and `mcpServers` JSON snippet.
2. In `tests/test_rust_mcp.py`, add checks verifying:
   - Exact byte-equality of `rust/assets/plugin/*` against `.agents/plugins/marketplace.json` and `plugins/apexuse/`.
   - `apexuse-mcp probe --mock` succeeds cleanly (`ok == true`, `tools_count == 6`).
   - `apexuse-mcp install codex-plugin --dry-run` / `--write --probe --mock` creates `.agents/plugins/marketplace.json` & plugin cache `.mcp.json` / `.codex/config.toml`, `install all --write` preserves active `codex-plugin`, and `install codex --write` switches back cleanly.
   - `apexuse-mcp onboard --mock` and `apexuse-mcp doctor --onboard --mock --check` exit `0` with structured JSON.

- [ ] **Step 2: Run Node & Python tests to verify failure before adding implementations**

Run: `node --test npm/apexuse-mcp/test/launcher.test.js`
Expected: FAIL (missing `../scripts/postinstall.js`).

- [ ] **Step 3: Implement `postinstall.js`, `openai.yaml` + skill `references/`, docs, and rebuild release**

1. Create `npm/apexuse-mcp/scripts/postinstall.js` (zero dependencies, `try/catch` safe) and update `npm/apexuse-mcp/package.json` (`"scripts": { "postinstall": "node ./scripts/postinstall.js" }`, `"files"` includes `"scripts/"`, `"dependencies": {}` untouched).
2. Create `.agents/skills/apexuse/agents/openai.yaml` (`interface`: `display_name`, `short_description`, `default_prompt`) and concise reference docs in `.agents/skills/apexuse/references/{installation.md, usage.md, troubleshooting.md}`. Keep `.agents/skills/apexuse/SKILL.md` == `rust/assets/SKILL.md`.
3. Update `README.md`, `agent-configs/README.md`, `llms.txt`, and `npm/apexuse-mcp/README.md` with accurate v0.3.0 documentation for `install codex-plugin`, `--probe`, `doctor --onboard`, `install all`, and `npx skills add Jackpkn/apexuse`.
4. Build release (`cargo build --release --manifest-path rust/Cargo.toml`) so `tests/test_rust_mcp.py` runs against updated binary.

- [ ] **Step 4: Run complete verification suite**

Run:
- `xcrun clang -fobjc-arc -O2 -mmacosx-version-min=14.0 -Wall -Wextra -Werror -fsyntax-only rust/native/macos_bridge.m`
- `cargo check --manifest-path rust/Cargo.toml --all-targets`
- `cargo check --manifest-path rust/Cargo.toml --target x86_64-pc-windows-gnu --lib`
- `cargo check --manifest-path rust/Cargo.toml --target x86_64-unknown-linux-gnu --lib`
- `cargo test --manifest-path rust/Cargo.toml`
- `cargo publish --dry-run --manifest-path rust/Cargo.toml` (once committed cleanly)
- `cargo build --release --manifest-path rust/Cargo.toml`
- `node --test npm/apexuse-mcp/test/launcher.test.js`
- `.venv/bin/pytest -q`
Expected: 100% pass, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add npm/apexuse-mcp .agents/skills/apexuse README.md agent-configs/README.md llms.txt tests/test_rust_mcp.py
git commit -m "feat(npm,skills,docs,e2e): add postinstall banner, Codex skill metadata, docs, and E2E tests"
```
