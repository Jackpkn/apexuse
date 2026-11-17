# ApexUse v0.3.0 UX Polish — Codex Desktop Plugin, MCP Probe, macOS Onboarding & Packaging

**Branch:** `feat/v0.3.1-codex-plugin-onboarding` (forked from `main` at `444bb84`)
**Target Release Version:** `0.3.0` (unified across `Cargo.toml`, `npm/apexuse-mcp/package.json`, `pyproject.toml`, `server.json`, `plugins/apexuse/.codex-plugin/plugin.json`, and `env!("CARGO_PKG_VERSION")` in `rust/src/mcp.rs`)
**Size:** Project (touches Rust CLI/installer/assets, Objective-C macOS AppKit bridge, npm launcher package, Codex plugin & skill metadata, and E2E/CI tests)

---

## 1. Intent & Why

Make ApexUse as effortless to install, verify, onboard on macOS, and launch inside Codex Desktop and other coding agents as `iFurySt/open-computer-use`, while preserving every core ApexUse advantage:
- **Zero external Rust crate or npm runtime dependencies** (`rust/Cargo.toml` = 0 crates; `npm/apexuse-mcp/package.json` = `"dependencies": {}`).
- **Single cross-platform binary + lightweight npm launcher** (`apexuse-mcp`), avoiding paid Apple Developer ID Gatekeeper blocks or shifting ad-hoc CDHash permission resets across upgrades.
- **Backward-compatible entrypoints**: zero-argument `apexuse-mcp` stays a strict MCP stdio JSON-RPC server; `apexuse-mcp doctor --check` keeps its existing JSON exit-code contract.
- **ApexUse's speed & grounding core untouched**: batched AX + diffing (`state_mode`), single-round-trip `return_state: "diff"`, `verify_mode: "ax"`, `set_value`/`select_text`, semantic `target`, `run_actions` (24 steps), Set-of-Mark `[m1]..[mN]`, OCR, pure-Rust JPEG, and `AgentCursor`.

## 2. Approaches Considered & Calls Settled

1. **Selected — Option A (Single Binary + Host-Aware AppKit Permission Onboarding + Codex Desktop Plugin):**
   - Keep `apexuse-mcp` as one direct-stdio binary across macOS/Linux/Windows.
   - Build native macOS permission onboarding (`onboard` / `doctor --onboard`) directly in `rust/native/macos_bridge.m` using AppKit (already linked by `rust/build.rs`). Because macOS 14+ TCC attributes permissions for child stdio processes to their responsible host app (`Terminal`, `iTerm2`, `Codex`, `Cursor`, `Antigravity`), the wizard detects and displays the actual host/responsible process identity alongside live status and exact System Settings deep-links — honest and upgrade-safe with zero daemon sockets.
   - Add native Codex Desktop Plugin files, `install codex-plugin --write`, `--probe` live MCP handshake verification, `npm postinstall`, and full `npx skills add` metadata.
2. **Rejected — Option B (`~/Applications/ApexUse.app` Local Wrapper + LaunchServices Unix Socket Relay):**
   - Avoided because an ad-hoc-signed `.app` loses macOS TCC grants on every rebuild/upgrade CDHash change and requires maintaining a background socket relay daemon.
3. **Rejected — LaunchServices `.app` Release Bundle (`open-computer-use` style):**
   - Requires a paid Apple Developer ID + notarization to avoid Gatekeeper quarantine on downloaded `.app` archives (0 valid signing identities on this machine).

### Builder-Check Calls Settled
- **Version (`0.3.0`):** Keep `0.3.0` across all manifests including `plugins/apexuse/.codex-plugin/plugin.json`, and switch hardcoded `"0.3.0"` in `rust/src/mcp.rs:403` to `env!("CARGO_PKG_VERSION")`.
- **Screen Recording live polling:** `macos_bridge.m` polls Accessibility via `AXIsProcessTrusted()` and checks Screen Recording via a short-lived `current_exe() doctor` child process (bypassing `CGPreflightScreenCaptureAccess()`'s in-process `NO` cache), displays a clear restart note if macOS still requires host relaunch, and `onboard` only exits non-zero when `--check` is explicitly passed alongside missing grants.
- **`install all` with active `codex-plugin`:** If `~/.codex/config.toml` already has `[plugins."apexuse@apexuse-local"]` with `enabled = true`, `install all --write` refreshes the `codex-plugin` cache/marketplace/TOML cleanly in place instead of overwriting it with `[mcp_servers.apexuse]`. Explicit `install codex` switches back cleanly (setting `[plugins."apexuse@apexuse-local"].enabled = false` if present), and explicit `install codex-plugin` enables plugin mode while removing `[mcp_servers.apexuse*]`.

## 3. Goals, Non-Goals & Anti-Goals

### Goals
1. **Codex Desktop Plugin (`install codex-plugin`):** First-class plugin package with SVG icons, starter prompts, `.mcp.json`, marketplace registry, materialized `~/.codex/plugins/` cache & marketplace layout, safe TOML activation, and mutual conflict-free switching with `[mcp_servers.apexuse]`.
2. **Live Post-Install MCP Probe (`--probe` & `apexuse-mcp probe`):** Spawn the configured MCP command/args/env and verify JSON-RPC `initialize` + `notifications/initialized` + `tools/list` in pure Rust with a bounded timeout.
3. **Host-Aware macOS Permission Onboarding (`onboard` / `doctor --onboard`):** Interactive AppKit window identifying the actual responsible/host app, showing Accessibility and Screen Recording status, triggering OS prompts, deep-linking into exact System Settings panes, and polling freshness without falling prey to in-process `CGPreflightScreenCaptureAccess()` caching.
4. **Zero-Friction Discovery (`npm postinstall` + `npx skills add` Pack):** Helpful postinstall instructions and `agents/openai.yaml` + reference guides alongside `SKILL.md`.

### Non-Goals / Anti-Goals
- No SwiftPM toolchain requirement in `rust/build.rs` and zero external Rust/npm dependencies.
- No macOS "Locked Use" privileged `launchd` unlock broker or `SecurityAgent` authorization plugin.
- No LaunchServices Unix-socket proxy or local `~/Applications/ApexUse.app` wrapper.
- No separate Node Worker JS REPL fork (`js`/`js_reset`); Codex uses ApexUse's native 6-tool + `run_actions` MCP surface.
- No unprompted launch of GUI windows inside `doctor --check` or bare `apexuse-mcp` MCP stdio mode.

---

## 4. Detailed Architecture

### A. Codex Desktop Plugin (`plugins/apexuse`, `.agents/plugins`, `installer.rs`)

1. **Repository & Packaged Assets:**
   - `.agents/plugins/marketplace.json`:
     - Marketplace `"apexuse-local"` (`displayName: "ApexUse Local"`), entry `{ "name": "apexuse", "source": { "source": "local", "path": "./plugins/apexuse" }, "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" }, "category": "Productivity" }`.
   - `plugins/apexuse/.codex-plugin/plugin.json`:
     - `name: "apexuse"`, `version: "0.3.0"`, `description`, `author`, `mcpServers: "./.mcp.json"`, `interface`: `displayName: "ApexUse"`, `shortDescription`, `longDescription`, `category: "Productivity"`, `capabilities: ["Interactive", "Read", "Write"]`, `defaultPrompt` (3 starter desktop/AX/workflow prompts), `brandColor: "#0E7490"`, `composerIcon: "./assets/apexuse-small.svg"`, `logo: "./assets/apexuse.svg"`.
   - `plugins/apexuse/.mcp.json`:
     ```json
     {
       "mcpServers": {
         "apexuse": {
           "command": "npx",
           "args": ["-y", "apexuse-mcp"]
         }
       }
     }
     ```
   - `plugins/apexuse/assets/apexuse.svg` & `plugins/apexuse/assets/apexuse-small.svg`: lightweight geometric cursor + window vector SVGs (`#0E7490`).
   - **Crates.io Packaging Sync:** Mirror static plugin files into `rust/assets/plugin/` (loaded via `include_str!` in `installer.rs` just like `rust/assets/SKILL.md`), with `tests/test_rust_mcp.py` asserting byte-equality against `.agents/plugins/marketplace.json` and `plugins/apexuse/`.
   - **Cached Plugin `.mcp.json`:** When `install codex-plugin --write` materializes `~/.codex/plugins/cache/apexuse-local/apexuse/0.3.0/` (and marketplace `~/.codex/plugins/marketplaces/apexuse-local/`), it writes `.mcp.json` using the resolved command/args (`npx -y apexuse-mcp` when `--npx`, or resolved binary path, plus `"env": { "APEXUSE_POLICY": "<path>" }` when `--policy` is given) so installs from npm, `crates.io`, or source checkouts are self-contained.

2. **`installer.rs` + `main.rs` Integration:**
   - Explicit target `codex-plugin`: `apexuse-mcp install codex-plugin --write [--dry-run] [--npx] [--policy <path>] [--with-skill] [--probe]`.
   - Scope check: rejects `--scope project` cleanly (`"codex-plugin only supports --scope user"`).
   - TOML updates in `~/.codex/config.toml` (creating `.bak` backup on modifications, preserving top-level settings and sibling tables):
     - Upserts `[marketplaces.apexuse-local]` (`source_type = "local"`, `source = "<resolved ~/.codex/plugins/marketplaces/apexuse-local>"`) and `[plugins."apexuse@apexuse-local"]` (`enabled = true`).
     - Removes `[mcp_servers.apexuse]` and `[mcp_servers.apexuse.env]` so Codex does not register duplicate `"apexuse"` servers.
   - `install_all_with_env`: checks whether existing `~/.codex/config.toml` (user scope only) has an active `[plugins."apexuse@apexuse-local"]` (`enabled = true`); if so, preserves/updates `codex-plugin` instead of overwriting it with `[mcp_servers.apexuse]`.
   - `install_agent_with_env("codex", ...)`: if `[plugins."apexuse@apexuse-local"]` exists with `enabled = true` in `~/.codex/config.toml`, sets `enabled = false` in that section cleanly when switching to `[mcp_servers.apexuse]`.

### B. Live Post-Install MCP Handshake Probe (`--probe` & `apexuse-mcp probe`)

1. **Pure-Rust Stdio Probe (`probe_mcp_server`):**
   - Uses `std::process::Command` (`Stdio::piped()`), `std::io::{BufRead, BufReader, Write}`, and a reader thread + `std::sync::mpsc::channel` with bounded timeout (default `10s`, configurable via `--timeout-ms`).
   - Sends:
     1. `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"apexuse-installer","version":"0.3.0"}}}` -> asserts `id == 1`, no `error`, and `result.serverInfo.name == "apexuse-mcp"`.
     2. `{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}`
     3. `{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}` -> asserts `id == 2`, non-empty unique `result.tools` array (`inspect_app`, `visual_inspect`, `computer`, `desktop`, `screenshot`, `workflow_memory`).
   - Always kills and reaps (`kill()` + `wait()`) the child process on completion, error, or timeout.
2. **CLI Integration:**
   - Standalone command: `apexuse-mcp probe [--mock] [--npx] [--policy <path>] [--timeout-ms <n>]` -> prints JSON `{ ok, command, args, server_name, server_version, tools_count, tools }` (or JSON error on stderr with exit `1`).
   - Flag `--probe` on `apexuse-mcp install <agent|codex-plugin|all> ...`: runs probe once using the resolved runtime command/args/policy (after `--write` or `--dry-run`) and attaches `"probe": { ... }` to the JSON payload; exits `1` if probing fails. (`--probe --mock` supported cleanly for CLI tests alongside live real-binary probing.)

### C. Host-Aware macOS Permission Onboarding (`onboard` / `doctor --onboard`)

- **C ABI (`macos_bridge.h` / `macos_bridge.m` + `driver/macos.rs`):**
  - `char *apexuse_macos_onboard(int force, int auto_close);`
  - Fast path: checks `AXIsProcessTrusted()` and `CGPreflightScreenCaptureAccess()`. When `!force && ax && sr`, returns immediately without UI: `{"ok":true,"os":"macos","shown":false,"already_granted":true,"accessibility":true,"screen_recording":true}`.
  - GUI path (when missing permissions or `force != 0`):
    - Promotes `[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory]` and opens a native `NSWindow` (**"ApexUse — macOS Permissions"**).
    - Identifies the current/responsible macOS process name (walking responsible/parent `NSRunningApplication` name + `bundleIdentifier` or current executable basename so the user sees e.g. `Terminal`, `iTerm2`, `Codex`, or `Cursor` explicitly in the subtitle!).
    - Shows live **Accessibility** and **Screen Recording** status badges:
      - **Grant Accessibility** button: triggers `AXIsProcessTrustedWithOptions` (`kAXTrustedCheckOptionPrompt: @YES`) + opens `x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility`.
      - **Grant Screen Recording** button: triggers `CGRequestScreenCaptureAccess()` + opens `x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture`.
      - `0.75s` `NSTimer` + **Re-check** button: updates Accessibility directly via `AXIsProcessTrusted()` and updates Screen Recording by spawning a short-lived `doctor` subprocess on the current executable (bypassing `CGPreflightScreenCaptureAccess()`'s in-process `NO` cache). Shows a clear host-restart tip when Screen Recording remains `false` after toggling.
      - When both turn `YES` and `auto_close != 0`, closes cleanly after `0.4s`; also closes on **Done** or window close.
- **CLI Entrypoints (`main.rs`):**
  - `apexuse-mcp onboard [--force] [--no-auto-close] [--check] [--mock]`
  - `apexuse-mcp doctor --onboard [--force] [--no-auto-close] [--check] [--mock]`
  - Non-macOS OSes output JSON `{"ok":true,"os":"<os>","shown":false,"note":"macOS permission onboarding is only required on macOS"}` with exit `0`.
  - Plain `apexuse-mcp doctor` preserves its exact JSON fields and adds `"onboard_hint": "Run 'apexuse-mcp doctor --onboard' to open guided setup"` only on macOS when `!accessibility || !screen_recording`.

### D. `npm postinstall` Banner & Rich `npx skills add` Pack

1. **`npm/apexuse-mcp/scripts/postinstall.js`:**
   - Added to `"scripts": { "postinstall": "node ./scripts/postinstall.js" }` and `"files": [..., "scripts/"]` in `npm/apexuse-mcp/package.json` (`"dependencies": {}` preserved).
   - Pure Node built-ins wrapped in `try/catch` (never fails `npm install`); detects `process.platform === "darwin"` to highlight `npx -y apexuse-mcp doctor --onboard`, plus `npx -y apexuse-mcp install all --write --npx --with-skill --probe` and the copy-paste JSON snippet. Logic exported for unit tests in `npm/apexuse-mcp/test/launcher.test.js`.
2. **Rich Skill Metadata (`.agents/skills/apexuse/`):**
   - Keep `.agents/skills/apexuse/SKILL.md` byte-identical to `rust/assets/SKILL.md`.
   - Add `.agents/skills/apexuse/agents/openai.yaml` (`interface`: `display_name`, `short_description`, `default_prompt`) and concise references under `.agents/skills/apexuse/references/{installation.md, usage.md, troubleshooting.md}`.
   - Update `README.md`, `agent-configs/README.md`, `llms.txt`, and `npm/apexuse-mcp/README.md` with `codex-plugin`, `--probe`, `doctor --onboard`, and `npx skills add Jackpkn/apexuse` instructions.

## 5. Verification Strategy

- **Rust Unit Tests (`cargo test`):**
  - `installer.rs`: `codex-plugin` TOML upsert, `[mcp_servers.apexuse*]` removal, mutual switching `codex` <-> `codex-plugin`, `install all` preserving active `codex-plugin`, `--scope project` rejection, materialized marketplace/cache `.mcp.json` layout, `--probe` handshake passing & failing closed on wrong server name/bad JSON/timeout.
  - `mcp.rs`: `initialize` `serverInfo.version == env!("CARGO_PKG_VERSION")`.
- **macOS Native Check (`xcrun clang`):** `-fobjc-arc -Wall -Wextra -Werror -fsyntax-only` on `rust/native/macos_bridge.m` -> 0 warnings, 0 errors.
- **Cross-Platform Compile Checks (`cargo check --all-targets` + Windows `x86_64-pc-windows-gnu` + Linux `x86_64-unknown-linux-gnu`):** 0 warnings.
- **Node Tests (`node --test npm/apexuse-mcp/test/launcher.test.js`):** Existing 12 tests + `postinstall.js` banner formatting & platform hint tests.
- **Python E2E (`tests/test_rust_mcp.py`):** Sync checks for `rust/assets/SKILL.md` & `rust/assets/plugin/*` == repo files, `probe --mock`, `install codex-plugin --dry-run` / `--write`, `--probe`, and `onboard --mock` CLI checks.
- **Live macOS Interactive Check:** Verify `apexuse-mcp doctor --onboard --force` launches the native AppKit window cleanly and `apexuse-mcp probe` passes against the real binary.

## 6. Left to the Builder (Planning & Implementation)

- Exact Objective-C AppKit layout coordinates, typography, and colors inside the onboarding window controller in `macos_bridge.m`.
- Exact SVG paths/artwork for `apexuse.svg` and `apexuse-small.svg`.
- Whether stdio probe lives in `rust/src/probe.rs` or a focused module inside `rust/src/installer.rs`.
