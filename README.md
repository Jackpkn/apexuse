# ApexUse — Computer Use MCP Server for AI Agents (macOS, Windows & Linux)

[![CI](https://github.com/Jackpkn/apexuse/actions/workflows/ci.yml/badge.svg)](https://github.com/Jackpkn/apexuse/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/apexuse-mcp.svg)](https://crates.io/crates/apexuse-mcp)
[![npm](https://img.shields.io/npm/v/apexuse-mcp.svg)](https://www.npmjs.com/package/apexuse-mcp)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
[![Rust Zero Dependencies](https://img.shields.io/badge/Rust-0%20external%20crates-orange.svg)](./rust/Cargo.toml)
[![MCP](https://img.shields.io/badge/MCP-stdio%20server-teal.svg)](./server.json)

**ApexUse** (`apexuse-mcp`) is an open-source, local-first **Computer Use MCP server** written in pure Rust and native OS bridges. It gives AI coding agents — including **Antigravity**, **Claude Code**, **Claude Desktop**, **OpenAI Codex**, **Gemini CLI**, **Cursor**, **Windsurf**, **VS Code**, and **OpenCode** — full desktop and browser control over **macOS, Windows, and Linux** using native Accessibility trees, **Set-of-Mark (`[m1]..[mN]`)** visual grounding, direct native `set_value`/`select_text`, semantic `target` lookup, local OCR, compact JPEG screenshots, a non-hijacking `AgentCursor` overlay, batched `run_actions` (up to 24 steps), and local workflow memory.

Zero external LLM keys. Zero external Rust crate or npm runtime dependencies. Zero cloud relay.

---

## Why ApexUse vs. Alternative Computer-Use Tools?

| Capability | **ApexUse (`apexuse-mcp`)** | Screenshot-Only Computer Use | Browser-Only MCPs (Playwright / DevTools) |
| --- | --- | --- | --- |
| **App Coverage** | **Full OS**: native apps, Electron, terminals & any browser | Whole screen | Browser tabs only |
| **UI Grounding** | **Hybrid**: semantic Accessibility (`e1..eN`) + deterministic `target: {query, role}` + Set-of-Mark (`[m1]..[mN]`) + OCR | Pixel coordinates only | DOM only |
| **Form Input & Speed** | Native **`set_value`** / **`select_text`**, **`keep_snapshot`** form-ref reuse, **`run_actions`** (up to 24 steps) + adaptive **`verify_change`** | 1 LLM round-trip per slow key/click | Step-by-step DOM calls |
| **Image Bandwidth** | Pure-Rust baseline **JPEG** default (`quality=80`, `max_width=1280`) + opt-in **PNG** | Uncompressed full-res PNGs | Page screenshots |
| **Visual Pointer** | Floating transparent **`AgentCursor`** overlay (never warps macOS hardware cursor) | Teleports real user cursor | None / page focus |
| **Workflow Memory** | Records & replays structural macros via **`workflow_memory`** | Re-plans every run | Scripts must be coded |
| **Security Controls** | Native password/secure-field blocking, `set_value` fails closed on unverified coordinates, protected-app list, `STOP` kill switch, audit log | Sends full raw screen pixels | Web origin scope |

---

## Platform Support

| Platform | Backend Driver | Capabilities & Verification |
| --- | --- | --- |
| **macOS 14+ (`arm64` & `x86_64`)** | `MacosDriver` + `rust/native/macos_bridge.m` | `AXUIElement` (`set_value` / `select_text`), `ScreenCaptureKit` (width scaling & 1500ms window cache), Apple `Vision` OCR, `ApexUseCursorView` overlay, chunked per-PID `CGEvent`/`NSEvent`. **Live verified on macOS.** |
| **Windows 10 / 11 (`x86_64`)** | `WindowsDriver` (`rust/src/driver/windows.rs`) | Pure-Rust Win32 FFI (`IUIAutomation` COM `ValuePattern`/`TextPattern`, `PrintWindow`/`BitBlt`, WinRT `Windows.Media.Ocr`, batched `SendInput`, layered `AgentCursor`, `som_png` JPEG/PNG codec). **Cross-checked (`x86_64-pc-windows-gnu`) & built in Windows CI.** |
| **Linux (`x86_64`, X11 & Wayland)** | `LinuxDriver` (`rust/src/driver/linux.rs`) | Runtime `dlopen` `libX11` / `libXtst` + `AT-SPI2` (`EditableText`/`Text`/`gi.repository.Atspi`), Wayland helpers (`grim` / `spectacle` / `gnome-screenshot`, `wtype` / `ydotool`), `tesseract` OCR, `som_png` JPEG/PNG codec. **Cross-checked (`x86_64-unknown-linux-gnu`) & built in Linux CI.** |

---

## Quick Start

### 1. Run via `npx` (zero-config, no Rust toolchain required)

The [`apexuse-mcp`](./npm/apexuse-mcp/README.md) npm launcher uses **zero external npm dependencies** (Node.js `>=18`), downloads the matching `v0.3.0` binary from [GitHub Releases](https://github.com/Jackpkn/apexuse/releases), verifies its `.sha256` checksum before extraction, and caches it under `~/.apexuse/bin/v0.3.0/`:

```sh
# Wire all detected AI coding agents + install bundled SKILL.md
npx -y apexuse-mcp install all --write --npx --with-skill

# Verify OS accessibility & screen recording readiness
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

### 3. Check OS readiness & permissions

```sh
npx -y apexuse-mcp doctor --check
# or with a local source build:
./rust/target/release/apexuse-mcp doctor --check
```

- **macOS:** Enable **Accessibility** and **Screen Recording** for the terminal or IDE launching `apexuse-mcp` in **System Settings → Privacy & Security**. (`doctor --check` exits `1` if either permission is missing.)
- **Windows:** Uses built-in Win32 / UI Automation / WinRT APIs with no extra dependencies.
- **Linux:** Runs on X11 via `libX11`/`libXtst` (`dlopen`). For full semantic accessibility, OCR, and Wayland support install distro packages as needed (e.g., `python3-gi`, `gir1.2-atspi-2.0`, `tesseract-ocr`, `xdotool`, or `grim`/`wtype`/`ydotool` on Wayland).

### 4. Wire your AI coding agent (`install`)

`apexuse-mcp install` auto-merges safely across **9 clients** (`antigravity`, `gemini`, `claude`, `claude-desktop`, `codex`, `cursor`, `windsurf`, `vscode`, `opencode`) or `all`:

```sh
# Preview without touching disk
npx -y apexuse-mcp install cursor --dry-run --npx

# Safely merge into existing config (creates .bak backup) + install SKILL.md
npx -y apexuse-mcp install all --write --npx --with-skill

# Project-scoped wiring with custom policy
npx -y apexuse-mcp install claude --write --npx --scope project --policy ./policy.autonomous.yaml
```

- Existing JSON files are validated before modification (invalid/commented JSON aborts cleanly without touching disk), non-empty changed files are backed up to `<file>.bak`, and Codex `config.toml` updates preserve top-level settings and sibling `[mcp_servers.*]` tables.
- `--with-skill` writes [`.agents/skills/apexuse/SKILL.md`](./.agents/skills/apexuse/SKILL.md) to standard skill directories (`~/.agents/skills/apexuse/SKILL.md`, `~/.claude/skills/apexuse/SKILL.md`, `~/.codex/skills/apexuse/SKILL.md`, or `.agents/` / `.claude/` in project scope).

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

## CLI Commands (`config`, `call`, `doctor`, `--version`)

```sh
apexuse-mcp --version
apexuse-mcp doctor [--check] [--mock]
apexuse-mcp install <agent|all> [--write] [--dry-run] [--scope user|project] [--npx] [--policy <path>] [--with-skill]
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

Launcher wrapper options (`npx -y apexuse-mcp`):
- `--use-system`: allow `PATH` fallback if no cached binary exists (stripped before forwarding args to `apexuse-mcp`).
- `APEXUSE_BIN`: override with a specific local `apexuse-mcp` binary path.

---

## MCP Tools Reference (6 Tools)

| Tool | Actions & Key Parameters |
| --- | --- |
| `inspect_app` | Read-only Accessibility snapshot for `app` (bundle ID, executable, app name, or `"desktop"`), optional `window` index and `query` filter. Returns `e1..eN` refs & workflow hints without taking a screenshot. |
| `visual_inspect` | Captures an app window or full `"desktop"`, fuses Accessibility + OCR (`ocr="auto"\|"fast"\|"accurate"\|"off"`, `max_marks` `1..=150`), and returns an annotated **JPEG** (default `format="jpeg"`, `quality=80`, `max_width=1280`) or opt-in **PNG** (`format="png"`) with `[m1]..[mN]` mark IDs. |
| `computer` | `inspect`, `click`, `double_click`, `right_click`, `hover`, `drag`, `type`, `set_value` (direct native text write/clear on verified editable controls; `text: ""` clears; fails closed on unverified coordinates/OCR-only marks), `select_text` (optional `start`/`end` offsets), `press_key`, `scroll` (`ref`/`mark`/`x,y`), `run_actions` (up to **24** chained steps including inline `set_value`, `select_text`, `wait_for`, `inspect`, and `target`), `wait_for` (local AX/OCR poll up to 60s), `confirm`. Supports semantic `target: { query, role?, fresh? }`, `keep_snapshot`, `verify_change`, `max_retries` (`0..=3`), `settle_ms`, `poll_interval_ms`, `retry_delay_ms`, `require_change`. |
| `desktop` | `list_applications`, `launch_application`, `activate_application`, `open_url`, `visual_click`, `double_click`, `right_click`, `hover`, `drag` (supports `mark`, `ref`, `target`, or `x`/`y`). |
| `screenshot` | Unannotated JPEG (default) or PNG (`format="png"`) capture of the largest window of a permitted running app after safety preflight. |
| `workflow_memory` | `list`, `lookup`, `record`, `replay` (with optional `step_args`), `clear` stored in `~/.apexuse/memory/recipes.json` (never stores typed/`set_value` secrets). |

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

Built-in safety guardrails enforced regardless of policy:
- Protected app denial (`password`, `1password`, `bitwarden`, `keychain`, `wallet`, `bank`, etc.)
- Native password / secure text field redaction (`AXSecureTextField`, `CurrentIsPassword`, `AT-SPI PASSWORD_TEXT`, bullet-mask checks)
- Fail-closed `set_value`: rejects raw `(x, y)` or OCR-only targets (`target_identity_unverified`) unless OS Accessibility verifies an editable control
- Clipboard paste blocking (`Cmd+V` / `Ctrl+V` rejected; use `set_value` or `type`)
- Emergency kill switch: create `~/.apexuse/STOP` to halt all desktop mutations immediately

---

## FAQ

**Does ApexUse need an OpenAI, Anthropic, or Google API key?**
No. `apexuse-mcp` is a pure local MCP stdio binary with zero built-in LLM calls. Your existing AI agent (Antigravity, Claude Code, Codex, Gemini CLI, Cursor, Windsurf, VS Code, OpenCode, etc.) drives it directly over MCP.

**How does ApexUse stay fast and token-efficient compared to traditional Computer Use?**
1. `inspect_app` reads compact semantic UI elements (`e1..eN`) with zero image encoding.
2. `set_value` and `select_text` mutate verified inputs directly via native Accessibility instead of 10ms/character keystroke loops.
3. `keep_snapshot` (`reuse_form_refs: true`) preserves element refs and marks across form field edits using `<2ms` window-state verification.
4. `visual_inspect` and `screenshot` default to scaled baseline JPEG (`quality=80`, `max_width=1280`, pure Rust) instead of huge PNGs, and `ocr="off"` returns instant AX-only visual marks.
5. `computer(action="run_actions")` chains up to **24** steps (including inline `set_value`, `wait_for`, and semantic `target`) inside one model turn, with adaptive early-exit `verify_change` polling (`30ms` cadence).

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
├── .agents/skills/apexuse/SKILL.md  # Embedded agent skill instructions for ApexUse MCP tools
├── .github/workflows/ci.yml         # 3-OS CI matrix (macOS, Ubuntu Xvfb/AT-SPI, Windows)
├── .github/workflows/release.yml    # Automated macOS (arm64/x86_64), Linux, Windows releases
├── agent-configs/README.md          # Client setup snippets & auto-wiring targets (9 agents)
├── llms.txt                         # AI search & crawler index summary
├── npm/apexuse-mcp/                 # Zero-dep npx launcher (Node >= 18, SHA-256 verified)
├── policy.yaml                      # Default direct policy
├── policy.autonomous.yaml           # Autonomous direct preset
├── pyproject.toml                   # Python MCP E2E test configuration
├── server.json                      # Official MCP Registry manifest
├── rust/
│   ├── Cargo.toml                   # apexuse-mcp crate (0 external dependencies)
│   ├── build.rs                     # Compiles Objective-C macOS bridge (arm64/x86_64)
│   ├── native/macos_bridge.{h,m}    # AXUIElement, SCKit, Vision OCR, AgentCursor, events
│   └── src/
│       ├── config.rs                # Persistent ~/.apexuse/config.json loader & validator
│       ├── driver/                  # OsDriver trait + common.rs + macos.rs + windows.rs + linux.rs
│       ├── gateway.rs               # Snapshots, policy, set_value, semantic target, run_actions
│       ├── installer.rs             # Safe multi-agent JSON/TOML auto-merger & SKILL.md installer
│       ├── json.rs                  # Zero-dependency JSON parser/serializer
│       ├── main.rs                  # CLI entrypoint (stdio, doctor, install, config, call)
│       ├── mcp.rs                   # JSON-RPC 2.0 MCP stdio server & tool schemas
│       ├── memory.rs                # Structural workflow memory
│       ├── policy.rs                # App/field security filters, rate limiter, confirmations
│       ├── som.rs                   # Set-of-Mark AX + OCR fusion
│       └── som_png.rs               # Pure-Rust PNG & baseline JPEG codecs, scaler, SoM renderer
└── tests/test_rust_mcp.py           # End-to-end MCP stdio & CLI integration tests
```

## Registry & License

- MIT License — see [LICENSE](./LICENSE)
- Official MCP Registry: mcp-name: io.github.Jackpkn/apexuse
