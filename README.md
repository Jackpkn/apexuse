# ApexUse — Computer Use MCP Server for AI Agents (macOS, Windows & Linux)

[![CI](https://github.com/Jackpkn/apexuse/actions/workflows/ci.yml/badge.svg)](https://github.com/Jackpkn/apexuse/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/apexuse-mcp.svg)](https://crates.io/crates/apexuse-mcp)
[![npm](https://img.shields.io/npm/v/apexuse-mcp.svg)](https://www.npmjs.com/package/apexuse-mcp)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
[![Rust Zero Dependencies](https://img.shields.io/badge/Rust-0%20external%20crates-orange.svg)](./rust/Cargo.toml)
[![MCP](https://img.shields.io/badge/MCP-stdio%20server-teal.svg)](./server.json)

**ApexUse** (`apexuse-mcp`) is an open-source, local-first **Computer Use MCP server** written in pure Rust and native OS bridges. It gives AI coding agents — including **Antigravity**, **Claude Code**, **Claude Desktop**, **OpenAI Codex**, **Gemini CLI**, **Cursor**, **Windsurf**, **VS Code**, and **OpenCode** — full desktop and browser control over **macOS, Windows, and Linux** using batched native Accessibility trees with incremental AX diffing (`state_mode`), inline post-action `return_state: "diff"`, **Set-of-Mark (`[m1]..[mN]`)** visual grounding, direct native `set_value`/`select_text`, semantic `target` lookup, local OCR, compact JPEG screenshots, a non-hijacking `AgentCursor` overlay, batched `run_actions` (up to 24 steps), and local workflow memory.

Zero external LLM keys. Zero external Rust crate or npm runtime dependencies. Zero cloud relay.

---

## Why ApexUse vs. Alternative Computer-Use Tools?

| Capability | **ApexUse (`apexuse-mcp`)** | Screenshot-Only Computer Use | Browser-Only MCPs (Playwright / DevTools) |
| --- | --- | --- | --- |
| **App Coverage** | **Full OS**: native apps, Electron, terminals & any browser | Whole screen | Browser tabs only |
| **UI Grounding** | **Hybrid AX-first**: batched Accessibility (`e1..eN`, `URL=`, Chrome/Electron tables) + incremental AX diffs + deterministic `target: {query, role}` + Set-of-Mark (`[m1]..[mN]`) + OCR | Pixel coordinates only | DOM only |
| **Form Input & Speed** | Native **`set_value`** / **`select_text`**, **`keep_snapshot`** form-ref reuse, **`return_state: "diff"`** (1-round-trip action + diff), **`run_actions`** (up to 24 steps) + **`verify_mode: "ax"`** | 1 LLM round-trip per slow key/click | Step-by-step DOM calls |
| **Image Bandwidth** | Pure-Rust baseline **JPEG** default (`quality=80`, `max_width=1280`) + opt-in **PNG** | Uncompressed full-res PNGs | Page screenshots |
| **Visual Pointer** | Floating transparent **`AgentCursor`** overlay (never warps macOS hardware cursor) | Teleports real user cursor | None / page focus |
| **Workflow Memory** | Records & replays structural macros via **`workflow_memory`** | Re-plans every run | Scripts must be coded |
| **Security Controls** | Native password/secure-field blocking, `set_value` fails closed on unverified coordinates, protected-app list, `STOP` kill switch, audit log | Sends full raw screen pixels | Web origin scope |

---

## Platform Support

| Platform | Backend Driver | Capabilities & Verification |
| --- | --- | --- |
| **macOS 14+ (`arm64` & `x86_64`)** | `MacosDriver` + `rust/native/macos_bridge.m` | Batched `AXUIElementCopyMultipleAttributeValues` IPC (`kAXURLAttribute` `URL=`, Chromium/Electron `AXTable`/`AXRow`/`AXCell`/`AXColumnHeader`/`AXTab`, `set_value` / `select_text`), `ScreenCaptureKit` (width scaling & 1500ms window metadata cache), Apple `Vision` OCR, `ApexUseCursorView` overlay, chunked per-PID `CGEvent`/`NSEvent`. **Live verified on macOS.** |
| **Windows 10 / 11 (`x86_64`)** | `WindowsDriver` (`rust/src/driver/windows.rs`) | Pure-Rust Win32 FFI (`IUIAutomation` COM `ValuePattern`/`TextPattern`, `PrintWindow`/`BitBlt`, WinRT `Windows.Media.Ocr`, batched `SendInput`, layered `AgentCursor`, `som_png` JPEG/PNG codec). **Cross-checked (`x86_64-pc-windows-gnu`) & CI-tested; real-desktop benchmarks in progress.** |
| **Linux (`x86_64`, X11 & Wayland)** | `LinuxDriver` (`rust/src/driver/linux.rs`) | Runtime `dlopen` `libX11` / `libXtst` + `AT-SPI2` (`EditableText`/`Text`/`gi.repository.Atspi`), Wayland helpers (`grim` / `spectacle` / `gnome-screenshot`, `wtype` / `ydotool`), `tesseract` OCR, `som_png` JPEG/PNG codec. **Cross-checked (`x86_64-unknown-linux-gnu`) & CI-tested under Xvfb; real-DE validation in progress.** |

---

## Quick Start

### 1. Run via `npx` (zero-config, no Rust toolchain required)

The [`apexuse-mcp`](./npm/apexuse-mcp/README.md) npm launcher uses **zero external npm dependencies** (Node.js `>=18`), downloads the matching `v0.3.0` binary from [GitHub Releases](https://github.com/Jackpkn/apexuse/releases), verifies its `.sha256` checksum before extraction, and caches it under `~/.apexuse/bin/v0.3.0/` (installing via `npm install -g apexuse-mcp` prints a platform-aware `postinstall` setup banner):

```sh
# Wire all detected AI coding agents, install SKILL.md & verify live MCP handshake
npx -y apexuse-mcp install all --write --npx --with-skill --probe

# Or install the Codex Desktop App Plugin directly
npx -y apexuse-mcp install codex-plugin --write --npx --probe

# macOS: open host-aware Accessibility & Screen Recording onboarding UI
npx -y apexuse-mcp doctor --onboard

# Cross-platform: verify OS accessibility & screen recording readiness (headless)
npx -y apexuse-mcp doctor --check
```

### 2. Alternative installs (`crates.io`, GitHub Releases, or source)

- Download prebuilt archives (`macos-arm64`, `macos-x86_64`, `linux-x86_64`, `windows-x86_64`) from [GitHub Releases](https://github.com/Jackpkn/apexuse/releases).
- Install from [crates.io](https://crates.io/crates/apexuse-mcp):
  ```sh
  cargo install apexuse-mcp
  ```
- Or clone and build from source (requires Rust, plus Xcode Command Line Tools on macOS):
  ```sh
  git clone https://github.com/Jackpkn/apexuse.git
  cd apexuse
  cargo build --release --manifest-path rust/Cargo.toml
  ```

### 3. Check OS readiness & guided macOS onboarding

```sh
# macOS guided AppKit onboarding window (or `apexuse-mcp onboard`)
npx -y apexuse-mcp doctor --onboard

# Headless readiness check across macOS, Windows, and Linux
npx -y apexuse-mcp doctor --check
# or with a local source build:
./rust/target/release/apexuse-mcp doctor --check
```

- **macOS:** Enable **Accessibility** and **Screen Recording** for the host terminal or IDE launching `apexuse-mcp` (`Terminal`, `iTerm2`, `Codex`, `Cursor`, `Antigravity`, etc.) in **System Settings → Privacy & Security**. Run `doctor --onboard` (`onboard [--force] [--no-auto-close]`) to detect the host app, jump straight to System Settings, and poll status live (if Screen Recording stays unset after toggling and clicking *Later*, restart the host app). `doctor --check` never spawns GUI windows and exits `1` when either grant is missing (emitting `"onboard_hint"` in JSON).
- **Windows:** Uses built-in Win32 / UI Automation / WinRT APIs with no extra dependencies.
- **Linux:** Runs on X11 via `libX11`/`libXtst` (`dlopen`). For full semantic accessibility, OCR, and Wayland support install distro packages as needed (e.g., `python3-gi`, `gir1.2-atspi-2.0`, `tesseract-ocr`, `xdotool`, or `grim`/`spectacle`/`gnome-screenshot` and `wtype`/`ydotool` on Wayland).

### 4. Wire your AI coding agent (`install`, `codex-plugin`, `probe`, `skills`)

`apexuse-mcp install` auto-merges safely across **9 clients** (`antigravity`, `gemini`, `claude`, `claude-desktop`, `codex`, `cursor`, `windsurf`, `vscode`, `opencode`), **`codex-plugin`** (Codex Desktop App Plugin), or **`all`**:

```sh
# Preview without touching disk
npx -y apexuse-mcp install cursor --dry-run --npx

# Safely merge into detected configs (creates .bak backup), install SKILL.md, and probe MCP
npx -y apexuse-mcp install all --write --npx --with-skill --probe

# Install Codex Desktop Plugin (marketplace + 0.3.0 cache + ~/.codex/config.toml) and probe
npx -y apexuse-mcp install codex-plugin --write --npx --probe

# Standalone live MCP initialize + tools/list probe
npx -y apexuse-mcp probe --npx

# Project-scoped wiring with custom policy
npx -y apexuse-mcp install claude --write --npx --scope project --policy ./policy.autonomous.yaml

# Install the rich agent skill pack (SKILL.md + agents/openai.yaml + references/)
npx skills add Jackpkn/apexuse -g -a codex --skill apexuse -y
```

- Existing JSON files are validated before modification (invalid/commented JSON aborts cleanly without touching disk), non-empty changed files are backed up to `<file>.bak`, and Codex `config.toml` updates preserve top-level settings and sibling tables.
- **Codex Plugin vs. Direct MCP Server:** `install codex-plugin --write` materializes `~/.codex/plugins/marketplaces/apexuse-local/` and `~/.codex/plugins/cache/apexuse-local/apexuse/0.3.0/`, activates `[marketplaces.apexuse-local]` + `[plugins."apexuse@apexuse-local"]`, and removes `[mcp_servers.apexuse*]` to avoid duplicate registrations. When `[plugins."apexuse@apexuse-local"]` (`enabled = true`) is active, `install all --write` preserves and refreshes `codex-plugin` in place; running `install codex --write` switches back cleanly (`enabled = false` + `[mcp_servers.apexuse]`).
- `--probe [--timeout-ms <n>] [--mock]` runs the pure-Rust MCP stdio handshake (`initialize`, `notifications/initialized`, `tools/list`) after install (`--mock` only applies to the temporary probe subprocess and is never written into configs or `.mcp.json`).
- `--with-skill` writes [`.agents/skills/apexuse/SKILL.md`](./.agents/skills/apexuse/SKILL.md) to standard skill directories (`~/.agents/skills/apexuse/SKILL.md`, `~/.claude/skills/apexuse/SKILL.md`, `~/.codex/skills/apexuse/SKILL.md`, or `.agents/` / `.claude/` in project scope). Use `npx skills add Jackpkn/apexuse` to install the full skill pack including [`agents/openai.yaml`](./.agents/skills/apexuse/agents/openai.yaml) and [`references/`](./.agents/skills/apexuse/references/installation.md).

<details>
<summary>Copy-paste MCP snippets (`npx -y apexuse-mcp`)</summary>

- **Antigravity / Gemini CLI / Cursor / Windsurf / Claude Desktop** (`"mcpServers"`):
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
- **Claude Code:**
  ```sh
  claude mcp add apexuse -- npx -y apexuse-mcp
  ```
- **OpenAI Codex** (`~/.codex/config.toml`):
  ```toml
  [mcp_servers.apexuse]
  command = "npx"
  args = ["-y", "apexuse-mcp"]
  startup_timeout_sec = 60
  tool_timeout_sec = 180
  ```
- **VS Code** (`mcp.json` — `"servers"`):
  ```json
  {
    "servers": {
      "apexuse": {
        "type": "stdio",
        "command": "npx",
        "args": ["-y", "apexuse-mcp"]
      }
    }
  }
  ```
- **OpenCode** (`opencode.json` — `"mcp"`):
  ```json
  {
    "mcp": {
      "apexuse": {
        "type": "local",
        "command": ["npx", "-y", "apexuse-mcp"],
        "enabled": true
      }
    }
  }
  ```

See [agent-configs/README.md](./agent-configs/README.md) for full paths across macOS, Windows, and Linux.

</details>

---

## CLI Commands (`config`, `call`, `doctor`, `onboard`, `probe`, `install`, `--version`)

```sh
apexuse-mcp --version
apexuse-mcp doctor [--onboard] [--force] [--no-auto-close] [--check] [--mock]
apexuse-mcp onboard [--force] [--no-auto-close] [--check] [--mock]
apexuse-mcp probe [--mock] [--npx] [--policy <path>] [--timeout-ms <n>]
apexuse-mcp install <agent|codex-plugin|all> [--write] [--dry-run] [--scope user|project] [--npx] [--policy <path>] [--with-skill] [--probe] [--timeout-ms <n>] [--mock]
apexuse-mcp config [show | get <key> | set <key> <val> | reset]
apexuse-mcp call <tool> [--args '<json>'] [--save-image <path>] [--mock]
apexuse-mcp call (--calls '<json-array>' | --calls-file <path>) [--sleep-ms <n>] [--save-image <path>] [--mock]
```

### Persistent Configuration (`~/.apexuse/config.json`)

Override defaults persistently via `apexuse-mcp config set <key> <value>` (or `APEXUSE_CONFIG`):

| Key | Default | Range / Values | Purpose |
| --- | --- | --- | --- |
| `image.format` | `"jpeg"` | `"jpeg"` \| `"png"` | Default screenshot/SoM image encoding (`visual_inspect`, `screenshot`). |
| `image.quality` | `80` | `1..=100` | Baseline JPEG quality. |
| `image.max_width` | `1280` | `640..=2560` | Maximum image pixel width before downscaling. |
| `timing.settle_ms` | `80` | `10..=3000` | Default `verify_change` polling window. |
| `timing.poll_interval_ms` | `30` | `10..=500` | Early-exit poll cadence during `verify_change`. |
| `timing.key_delay_us` | `500` | `0..=50000` | Inter-chunk delay for fast Unicode fallback typing. |
| `snapshot.reuse_form_refs` | `true` | `true` \| `false` | Preserve `e1..eN` & `[m1]..[mN]` across non-navigating edits (`type`, `set_value`, `select_text`). |
| `snapshot.ax_diff` | `true` | `true` \| `false` | Auto-diff repeat `inspect_app` / inline `inspect` trees of the same app/window (`state_mode`: `"full"` \| `"diff"` \| `"unchanged"`). |
| `snapshot.include_bounds` | `false` | `true` \| `false` | Include `@(x,y,wxh)` element bounds in AX outlines (`inspect_app`, `return_state`). |

Launcher wrapper options (`npx -y apexuse-mcp`):
- `--use-system`: allow `PATH` fallback if no cached binary exists (stripped before forwarding args to `apexuse-mcp`).
- `APEXUSE_BIN`: override with a specific local `apexuse-mcp` binary path.

---

## MCP Tools Reference (6 Tools)

| Tool | Actions & Key Parameters |
| --- | --- |
| `inspect_app` | AX-first read-only snapshot for `app` (bundle ID, executable, app name, or `"desktop"`), optional `window` index, `query` filter, `diff` (default `config.snapshot.ax_diff` `true`), and `include_bounds` (default `config.snapshot.include_bounds` `false` to omit `@(x,y,wxh)`). Exposes `e1..eN` refs, `URL=` on windows/`WebArea`/links, Chrome/Electron table/tab structure, `state_mode` (`"full" \| "diff" \| "unchanged" \| "filtered"`), and workflow hints without taking a screenshot. |
| `visual_inspect` | Captures an app window or full `"desktop"`, fuses Accessibility + OCR (`ocr="auto"\|"fast"\|"accurate"\|"off"`, `max_marks` `1..=150`), and returns an annotated **JPEG** (default `format="jpeg"`, `quality=80`, `max_width=1280`) or opt-in **PNG** (`format="png"`) with `[m1]..[mN]` mark IDs. |
| `computer` | `inspect` (supports `diff`, `include_bounds`, `window`, `query`), `click`, `double_click`, `right_click`, `hover`, `drag`, `type`, `set_value` (direct native text write/clear on verified editable controls; `text: ""` clears; fails closed on unverified coordinates/OCR-only marks), `select_text` (optional `start`/`end` offsets), `press_key`, `scroll` (`ref`/`mark`/`x,y`), `run_actions` (up to **24** chained steps including inline `set_value`, `select_text`, `wait_for`, `inspect`, `target`, per-step `verify_mode`/`diff`/`include_bounds`, and `failed_step`/`executed`/`results` telemetry on mid-batch failures), `wait_for` (local AX/OCR poll up to 60s), `confirm`. Supports `return_state="none"\|"diff"\|"full"` (attaches post-action AX diff/tree inline and keeps new refs live in 1 round-trip), `include_bounds`, `verify_mode="auto"\|"ax"\|"visual"` (`"ax"` checks Accessibility changes with zero screenshots), semantic `target: { query, role?, fresh? }`, `keep_snapshot`, `verify_change`, `max_retries` (`0..=3`), `settle_ms`, `poll_interval_ms`, `retry_delay_ms`, `require_change`. |
| `desktop` | `list_applications`, `launch_application`, `activate_application`, `open_url`, `visual_click`, `double_click`, `right_click`, `hover`, `drag` (supports `mark`, `ref`, `target`, or `x`/`y`). Supports `return_state="none"\|"diff"\|"full"`, `include_bounds`, `verify_change`, `verify_mode="auto"\|"ax"\|"visual"`, `max_retries`, `settle_ms`, `poll_interval_ms`, `retry_delay_ms`, `require_change`. |
| `screenshot` | Unannotated JPEG (default) or PNG (`format="png"`) capture of the largest window of a permitted running app after safety preflight. |
| `workflow_memory` | `list`, `lookup`, `record` (supports optional `expected_domain`, `expected_window_contains`, and per-step `verify_change`), `replay` (with precondition validation, optional `step_args`, and per-step failure reporting), `clear` stored in `~/.apexuse/memory/recipes.json` (never stores typed/`set_value` secrets). |

---

## Policy, Safety & Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `APEXUSE_POLICY` | `./policy.yaml` (then ancestor search) | Policy YAML (`policy.yaml` or `policy.autonomous.yaml`). |
| `APEXUSE_STATE_DIR` | `~/.apexuse` | Directory for `bin/`, `config.json`, `logs/audit.jsonl`, `memory/recipes.json`, and `STOP`. |
| `APEXUSE_CONFIG` | `$APEXUSE_STATE_DIR/config.json` | Override persistent runtime config file path. |
| `APEXUSE_BIN` | *(unset)* | Explicit binary path override for the `npx` launcher. |
| `APEXUSE_MOCK` | `0` | Set to `1` (or `--mock`) to run the deterministic in-memory `MockDriver`. |

*(Legacy `HANDSMCP_*` env vars remain supported as fallbacks.)*

Built-in safety & reliability guardrails:
- Protected app denial (`password`, `1password`, `bitwarden`, `keychain`, `wallet`, `bank`, etc.)
- Browser domain policy (`allowed_domains` / `blocked_domains` in `policy.yaml` enforced across `open_url`, `inspect_app`, `visual_inspect`, `screenshot`, and `workflow_memory` replay)
- Optional destructive-action confirmation (`confirm_destructive: true` in `policy.yaml` requires interactive confirmation tokens for irreversible labels like `Delete`, `Purchase`, `Transfer`, `Send`)
- Direct-mode element identity & window-state checks (`probe_point_safe` + process/window-count verification) to reject stale `ref` targets when UI layout shifts before click or `set_value`
- Native password / secure text field redaction (`AXSecureTextField`, `CurrentIsPassword`, `AT-SPI PASSWORD_TEXT`, bullet-mask checks)
- Fail-closed `set_value`: rejects raw `(x, y)` or OCR-only targets (`target_identity_unverified`) unless OS Accessibility verifies an editable control
- Clipboard paste blocking (`Cmd+V` / `Ctrl+V` rejected; use `set_value` or `type`)
- Emergency kill switch: create `~/.apexuse/STOP` to halt all desktop mutations immediately

---

## FAQ

**Does ApexUse need an OpenAI, Anthropic, or Google API key?**
No. `apexuse-mcp` is a pure local MCP stdio binary with zero built-in LLM calls. Your existing AI agent (Antigravity, Claude Code, Codex, Gemini CLI, Cursor, Windsurf, VS Code, OpenCode, etc.) drives it directly over MCP.

**How does ApexUse stay fast and token-efficient compared to traditional Computer Use?**
1. `inspect_app` batches macOS AX attribute reads (`AXUIElementCopyMultipleAttributeValues`), omits `@(x,y,wxh)` bounds by default (`include_bounds: false`), exposes `URL=` and Chrome/Electron table structure, and auto-diffs repeat calls (`diff: true` -> `state_mode: "full" | "diff" | "unchanged"`) with zero image encoding.
2. Passing `return_state: "diff"` on `computer`, `desktop`, or `run_actions` executes the action and returns the updated AX diff with live `e1..eN` refs (`snapshot_active: true`, `snapshot_preserved: true`) in **one** MCP round-trip — no extra `inspect_app` call needed.
3. `set_value` and `select_text` mutate verified inputs directly via native Accessibility instead of 10ms/character keystroke loops, while `keep_snapshot` (`reuse_form_refs: true`) preserves refs across form edits.
4. `verify_mode: "ax"` verifies `verify_change: true` transitions strictly via Accessibility with zero screenshot overhead (`30ms` early-exit polling).
5. `visual_inspect` and `screenshot` default to scaled baseline JPEG (`quality=80`, `max_width=1280`, pure Rust) instead of huge PNGs, and `ocr="off"` returns instant AX-only visual marks.
6. `computer(action="run_actions")` chains up to **24** steps (including inline `set_value`, `wait_for`, and semantic `target`) inside a single model turn.

**Does it hijack my physical mouse cursor while working?**
On macOS, `ApexUse` renders an independent translucent `AgentCursor` overlay window and dispatches targeted events without warping your hardware cursor during normal visual feedback. On Windows, a layered click-through `AgentCursor` overlay tracks automated actions.

**How is ApexUse different from Playwright or browser DevTools MCP servers?**
Browser MCPs only control a single automated browser tab via CDP. ApexUse controls the real desktop across native OS apps, Electron apps, terminals, dialogs, and user browsers (Chrome, Dia, Safari, Firefox, Edge).

---

## Testing

```sh
# Rust unit & integration tests
cargo test --manifest-path rust/Cargo.toml

# Zero-dep npm launcher unit tests (node >= 18)
node --test npm/apexuse-mcp/test/launcher.test.js

# End-to-end MCP stdio & CLI integration tests (requires python3 mcp + pytest)
.venv/bin/pytest -q
```

## Layout

```text
├── .agents/
│   ├── plugins/marketplace.json     # Codex local plugin marketplace catalog
│   └── skills/apexuse/              # Skill pack (SKILL.md, agents/openai.yaml, references/)
├── .github/workflows/ci.yml         # 3-OS CI matrix (macOS, Ubuntu Xvfb/AT-SPI, Windows)
├── .github/workflows/release.yml    # Automated macOS (arm64/x86_64), Linux, Windows releases
├── agent-configs/README.md          # Client setup snippets & auto-wiring targets (9 agents + plugin)
├── llms.txt                         # AI search & crawler index summary
├── npm/apexuse-mcp/                 # Zero-dep npx launcher + postinstall banner (Node >= 18)
├── plugins/apexuse/                 # Codex Desktop Plugin manifest, .mcp.json, and SVG icons
├── policy.yaml                      # Default direct policy
├── policy.autonomous.yaml           # Autonomous direct preset
├── pyproject.toml                   # Python MCP E2E test configuration
├── server.json                      # Official MCP Registry manifest
├── rust/
│   ├── Cargo.toml                   # apexuse-mcp crate (0 external dependencies)
│   ├── build.rs                     # Compiles Objective-C macOS bridge (arm64/x86_64)
│   ├── assets/                      # Embedded SKILL.md & plugin templates for crates.io
│   ├── native/macos_bridge.{h,m}    # AXUIElement, SCKit, Vision OCR, AgentCursor, onboarding UI
│   └── src/
│       ├── config.rs                # Persistent ~/.apexuse/config.json loader & validator
│       ├── driver/                  # OsDriver trait + common.rs + macos.rs + windows.rs + linux.rs
│       ├── gateway.rs               # Snapshots, policy, set_value, semantic target, run_actions
│       ├── installer.rs             # Safe multi-agent + codex-plugin JSON/TOML auto-merger
│       ├── json.rs                  # Zero-dependency JSON parser/serializer
│       ├── main.rs                  # CLI entrypoint (stdio, doctor, onboard, probe, install, config, call)
│       ├── mcp.rs                   # JSON-RPC 2.0 MCP stdio server & tool schemas
│       ├── memory.rs                # Structural workflow memory
│       ├── policy.rs                # App/field security filters, rate limiter, confirmations
│       ├── probe.rs                 # Pure-Rust bounded live MCP stdio handshake probe
│       ├── som.rs                   # Set-of-Mark AX + OCR fusion
│       └── som_png.rs               # Pure-Rust PNG & baseline JPEG codecs, scaler, SoM renderer
└── tests/test_rust_mcp.py           # End-to-end MCP stdio, asset-sync & CLI integration tests
```

## Registry & License

- MIT License — see [LICENSE](./LICENSE)
- Official MCP Registry: mcp-name: io.github.Jackpkn/apexuse
