# ApexUse — Computer Use MCP Server for AI Agents (macOS, Windows & Linux)

[![CI](https://github.com/Jackpkn/apexuse/actions/workflows/ci.yml/badge.svg)](https://github.com/Jackpkn/apexuse/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/apexuse-mcp.svg)](https://crates.io/crates/apexuse-mcp)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
[![Rust Zero Dependencies](https://img.shields.io/badge/Rust-0%20external%20crates-orange.svg)](./rust/Cargo.toml)
[![MCP](https://img.shields.io/badge/MCP-stdio%20server-teal.svg)](./server.json)

**ApexUse** (`apexuse-mcp`) is an open-source, local-first **Computer Use MCP server** written in pure Rust and native OS bridges. It gives AI coding agents — including **Claude Code**, **OpenAI Codex**, **Gemini CLI**, and **Antigravity** — full desktop and browser control over **macOS, Windows, and Linux** using native Accessibility trees, **Set-of-Mark (`[m1]..[mN]`)** visual grounding, local OCR, a non-hijacking `AgentCursor` overlay, batched `run_actions`, and local workflow memory.

Zero external LLM keys. Zero external Rust crate dependencies. Zero cloud relay.

---

## Why ApexUse vs. Alternative Computer-Use Tools?

| Capability | **ApexUse (`apexuse-mcp`)** | Screenshot-Only Computer Use | Browser-Only MCPs (Playwright / DevTools) |
| --- | --- | --- | --- |
| **App Coverage** | **Full OS**: native apps, Electron, terminals & any browser | Whole screen | Browser tabs only |
| **UI Grounding** | **Hybrid**: semantic Accessibility (`e1..eN`) + Set-of-Mark (`[m1]..[mN]`) + OCR | Pixel coordinates only | DOM only |
| **Multi-Step Speed** | **`run_actions`** (up to 12 local chained steps) + local **`wait_for`** polling | 1 LLM round-trip per click | Step-by-step tool calls |
| **Scroll & Verification** | Target-scoped `scroll` (`ref`/`mark`/`x,y`) + `verify_change` auto-retry | Blind scroll + manual re-screenshot | Page scroll only |
| **Visual Pointer** | Floating transparent **`AgentCursor`** overlay (never warps macOS hardware cursor) | Teleports real user cursor | None / page focus |
| **Workflow Memory** | Records & replays structural macros via **`workflow_memory`** | Re-plans every run | Scripts must be coded |
| **Security Controls** | Native password/secure-field blocking, protected-app list, `STOP` kill switch, audit log | Sends full raw screen pixels | Web origin scope |

---

## Platform Support

| Platform | Backend Driver | Capabilities & Verification |
| --- | --- | --- |
| **macOS 14+** | `MacosDriver` + `rust/native/macos_bridge.m` | `AXUIElement`, `ScreenCaptureKit`, Apple `Vision` OCR, `ApexUseCursorView` overlay, per-PID `CGEvent`/`NSEvent`. **Live verified on macOS.** |
| **Windows 10 / 11** | `WindowsDriver` (`rust/src/driver/windows.rs`) | Pure-Rust Win32 FFI (`IUIAutomation` COM, `PrintWindow`/`BitBlt`, WinRT `Windows.Media.Ocr`, `SendInput`, layered `AgentCursor`, `som_png` overlay). **Cross-checked (`x86_64-pc-windows-gnu`) & built in Windows CI; live Windows host test pending.** |
| **Linux (X11 & Wayland)** | `LinuxDriver` (`rust/src/driver/linux.rs`) | Runtime `dlopen` `libX11` / `libXtst` + `AT-SPI2` (`gi.repository.Atspi`), Wayland helpers (`grim` / `spectacle` / `gnome-screenshot`, `wtype` / `ydotool`), `tesseract` OCR, `som_png` overlay. **Cross-checked (`x86_64-unknown-linux-gnu`) & built in Linux CI; live Linux host test pending.** |

---

## Quick Start

### 1. Install or build from source

Download prebuilt binaries for **macOS (`arm64`)**, **Linux (`x86_64`)**, or **Windows (`x86_64`)** from [GitHub Releases](https://github.com/Jackpkn/apexuse/releases), or install directly from [crates.io](https://crates.io/crates/apexuse-mcp):

```sh
cargo install apexuse-mcp
```

Or clone and build directly (requires only the Rust toolchain, plus Xcode Command Line Tools on macOS):

```sh
git clone https://github.com/Jackpkn/apexuse.git
cd apexuse
cargo build --release --manifest-path rust/Cargo.toml
```

Output binary: `rust/target/release/apexuse-mcp` (or `apexuse-mcp.exe` on Windows).

### 2. Check OS readiness & permissions

```sh
./rust/target/release/apexuse-mcp doctor
```

- **macOS:** Enable **Accessibility** and **Screen Recording** for the terminal or IDE launching `apexuse-mcp` in **System Settings → Privacy & Security**.
- **Windows:** Uses built-in Win32 / UI Automation / WinRT APIs with no extra dependencies.
- **Linux:** Runs on X11 via `libX11`/`libXtst` (`dlopen`). For full semantic accessibility, OCR, and Wayland support install distro packages as needed (e.g., `python3-gi`, `gir1.2-atspi-2.0`, `tesseract-ocr`, `xdotool`, or `grim`/`wtype`/`ydotool` on Wayland).

### 3. Connect to your AI coding agent

Generate a ready-to-paste snippet for your client:

```sh
./rust/target/release/apexuse-mcp install antigravity
# Supported: antigravity | gemini | claude | codex
```

<details>
<summary>Copy-paste snippets for Antigravity, Claude Code, OpenAI Codex, and Gemini CLI</summary>

- **Claude Code:**
  ```sh
  claude mcp add apexuse --env APEXUSE_POLICY=/abs/path/to/apexuse/policy.autonomous.yaml -- /abs/path/to/apexuse/rust/target/release/apexuse-mcp
  ```
- **Antigravity / Gemini CLI** (`~/.gemini/antigravity/mcp_config.json` or `~/.gemini/settings.json`):
  ```json
  {
    "mcpServers": {
      "apexuse": {
        "command": "/abs/path/to/apexuse/rust/target/release/apexuse-mcp",
        "args": [],
        "env": {
          "APEXUSE_POLICY": "/abs/path/to/apexuse/policy.autonomous.yaml"
        }
      }
    }
  }
  ```
- **OpenAI Codex** (`~/.codex/config.toml`):
  ```toml
  [mcp_servers.apexuse]
  command = "/abs/path/to/apexuse/rust/target/release/apexuse-mcp"
  args = []
  startup_timeout_sec = 60
  tool_timeout_sec = 180

  [mcp_servers.apexuse.env]
  APEXUSE_POLICY = "/abs/path/to/apexuse/policy.autonomous.yaml"
  ```

See [agent-configs/README.md](./agent-configs/README.md) and the bundled skill at [`.agents/skills/apexuse/SKILL.md`](./.agents/skills/apexuse/SKILL.md).

</details>

---

## MCP Tools Reference (6 Tools)

| Tool | Actions & Key Parameters |
| --- | --- |
| `inspect_app` | Read-only Accessibility snapshot for `app` (bundle ID, executable, app name, or `"desktop"`), optional `window` index and `query` filter. Returns `e1..eN` refs & workflow hints without taking a screenshot. |
| `visual_inspect` | Captures an app window or full `"desktop"`, fuses Accessibility + OCR (`ocr="auto"\|"fast"\|"accurate"\|"off"`, `max_marks` `1..=150`), and returns an annotated PNG with `[m1]..[mN]` mark IDs. |
| `computer` | `inspect`, `click`, `double_click`, `right_click`, `hover` (`ref`/`mark`/`x,y`), `drag`, `type`, `press_key`, `scroll` (optional `ref`/`mark`/`x,y` target), `run_actions` (up to 12 batched steps + `inspect_after`), `wait_for` (local AX/OCR poll up to 60s), `confirm`. Supports `verify_change`, `max_retries` (`0..=3`), `settle_ms`, `retry_delay_ms`, `require_change`. |
| `desktop` | `list_applications`, `launch_application`, `activate_application`, `open_url`, `visual_click`, `double_click`, `right_click`, `hover`, `drag`. |
| `screenshot` | Unannotated PNG capture of the largest window of a permitted running app after safety preflight. |
| `workflow_memory` | `list`, `lookup`, `record`, `replay` (with optional `step_args`), `clear` stored in `~/.apexuse/memory/recipes.json`. |

---

## Policy, Safety & Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `APEXUSE_POLICY` | `./policy.yaml` (then ancestor search) | Policy YAML (`policy.yaml` or `policy.autonomous.yaml`). |
| `APEXUSE_STATE_DIR` | `~/.apexuse` | Directory for `logs/audit.jsonl`, `memory/recipes.json`, and `STOP`. |
| `APEXUSE_MOCK` | `0` | Set to `1` (or `--mock`) to run the deterministic in-memory `MockDriver`. |

*(Legacy `HANDSMCP_*` env vars remain supported as fallbacks.)*

Built-in safety guardrails enforced regardless of policy:
- Protected app denial (`password`, `1password`, `bitwarden`, `keychain`, `wallet`, `bank`, etc.)
- Native password / secure text field redaction (`AXSecureTextField`, `CurrentIsPassword`, `AT-SPI PASSWORD_TEXT`, bullet-mask checks)
- Clipboard paste blocking (`Cmd+V` / `Ctrl+V` rejected; use `computer(action="type")`)
- Emergency kill switch: create `~/.apexuse/STOP` to halt all desktop mutations immediately

---

## FAQ

**Does ApexUse need an OpenAI, Anthropic, or Google API key?**
No. `apexuse-mcp` is a pure local MCP stdio binary with zero built-in LLM calls. Your existing AI agent (Claude Code, Codex, Gemini CLI, Antigravity, etc.) drives it directly over MCP.

**How does ApexUse avoid the slow screenshot loop of traditional Computer Use?**
First, `inspect_app` reads compact semantic UI elements (`e1..eN`) without image encoding. Second, `visual_inspect` lets you set `ocr="off"` for instant AX-only visual marks. Third, `computer(action="run_actions")` batches up to 12 deterministic clicks, keystrokes, scrolls, and waits inside a single turn, and `computer(action="wait_for")` polls locally instead of burning model turns.

**Does it hijack my physical mouse cursor while working?**
On macOS, `ApexUse` renders an independent translucent `AgentCursor` overlay window and dispatches targeted events without warping your hardware cursor during normal visual feedback. On Windows, a layered click-through `AgentCursor` overlay tracks automated actions.

**How is ApexUse different from Playwright or browser DevTools MCP servers?**
Browser MCPs only control a single automated browser tab via CDP. ApexUse controls the real desktop across native OS apps, Electron apps, terminals, dialogs, and user browsers (Chrome, Dia, Safari, Firefox, Edge).

---

## Testing

```sh
# Rust unit & integration tests (38 tests)
cargo test --manifest-path rust/Cargo.toml

# End-to-end MCP stdio integration tests (requires python3 mcp + pytest)
.venv/bin/pytest -q
```

## Layout

```text
├── .agents/skills/apexuse/SKILL.md  # Agent skill instructions for ApexUse MCP tools
├── .github/workflows/ci.yml         # 3-OS CI matrix (macOS, Ubuntu Xvfb/AT-SPI, Windows)
├── .github/workflows/release.yml    # Automated macOS/Linux/Windows release binaries
├── agent-configs/README.md          # Client setup snippets (Antigravity, Gemini, Codex, Claude)
├── llms.txt                         # AI search & crawler index summary
├── policy.yaml                      # Default direct policy
├── policy.autonomous.yaml           # Autonomous direct preset
├── pyproject.toml                   # Python MCP E2E test configuration
├── server.json                      # Official MCP Registry manifest
├── rust/
│   ├── Cargo.toml                   # apexuse-mcp crate (0 external dependencies)
│   ├── build.rs                     # Compiles Objective-C macOS bridge on macOS
│   ├── native/macos_bridge.{h,m}    # AXUIElement, SCKit, Vision OCR, AgentCursor, events
│   └── src/
│       ├── driver/                  # OsDriver trait + common.rs + macos.rs + windows.rs + linux.rs
│       ├── gateway.rs               # Snapshots, policy enforcement, verify_change, run_actions
│       ├── json.rs                  # Zero-dependency JSON parser/serializer
│       ├── mcp.rs                   # JSON-RPC 2.0 MCP stdio server & tool schemas
│       ├── memory.rs                # Structural workflow memory
│       ├── policy.rs                # App/field security filters, rate limiter, confirmations
│       ├── som.rs                   # Set-of-Mark AX + OCR fusion
│       └── som_png.rs               # Pure-Rust PNG codec, inflate, pixel digest & SoM renderer
└── tests/test_rust_mcp.py           # End-to-end MCP stdio integration tests
```

## Registry & License

- MIT License — see [LICENSE](./LICENSE)
- Official MCP Registry: mcp-name: io.github.Jackpkn/apexuse
