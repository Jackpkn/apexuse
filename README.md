# ApexUse

**ApexUse** (`apexuse-mcp`) is a fast, local-first **Computer Use MCP server** built in Rust and native OS APIs (Objective-C on macOS, Win32/UIA/GDI on Windows, and AT-SPI2/X11/Wayland on Linux). It gives any MCP-compatible AI coding agent (Antigravity, Gemini CLI, Claude Code, Codex, and custom MCP clients) semantic Accessibility control, Set-of-Mark (`[m1]..[mN]`) visual grounding, native OCR, an independent floating `AgentCursor`, and local workflow memory over JSON-RPC stdio — with no external LLM dependency, zero external Rust crates, and no cloud service.

---

## Key Capabilities

- **6 Focused MCP Tools** (`inspect_app`, `visual_inspect`, `computer`, `desktop`, `screenshot`, `workflow_memory`)
- **Hybrid Accessibility + Set-of-Mark Visual Grounding**:
  - `inspect_app` reads compact semantic Accessibility trees (`e1..eN` refs, bounding boxes, workflow hints) without taking a screenshot.
  - `visual_inspect` captures windows or the full `"desktop"`, fuses Accessibility targets with OCR (`ocr="auto"|"fast"|"accurate"|"off"` via Apple Vision on macOS, WinRT OCR on Windows, and Tesseract on Linux), and renders clean `[m1]..[mN]` badges on the returned PNG.
- **Fast Multi-Step Execution & Local Polling**:
  - `computer(action="run_actions")` executes up to 12 deterministic steps (`click`, `type`, `press_key`, `scroll`, `sleep_ms`, `activate_application`, etc.) locally in a single turn with optional `inspect_after: true`.
  - `computer(action="wait_for")` polls locally (AX and/or OCR, optional `window` filter) for elements to appear or disappear up to 60 seconds without extra LLM round-trips.
  - `desktop(action="open_url")` opens `http`/`https` URLs directly in Chrome or a permitted browser/app.
- **Targeted Scroll & Change Verification**:
  - `computer(action="scroll")` accepts optional `ref`, `mark`, or `(x, y)` coordinates to scroll inner panes cleanly without warping the user's cursor.
  - Actions and `run_actions` steps support optional `verify_change`, `max_retries` (`0..=3`), `settle_ms`, `retry_delay_ms`, and `require_change` to compare pre/post Accessibility & pixel digests and automatically retry if nothing changed.
- **Independent Floating `AgentCursor` Overlay**:
  - Shows where the agent is pointing, gliding, clicking (with click ripple), typing, or scrolling via a translucent overlay window on macOS and Windows (`WS_EX_LAYERED | WS_EX_TRANSPARENT`), without hijacking the user's hardware cursor on macOS.
- **Local Workflow Memory (`workflow_memory`)**:
  - Records and replays reusable non-secret UI macros (`list`, `lookup`, `record`, `replay` with `step_args`, `clear`) stored in `~/.apexuse/memory/recipes.json`.
- **Configurable Policy & Hardened Safety Guardrails**:
  - Controlled via [`policy.yaml`](./policy.yaml) / [`policy.autonomous.yaml`](./policy.autonomous.yaml) (`confirmation: direct` or `required`, `normal | read_only | dry_run`, rate limits, `STOP` kill switch, protected-app and secure-field blocking).

---

## Platform Support

| Platform | Driver | Capabilities & Verification |
| --- | --- | --- |
| **macOS 14+** | `MacosDriver` + `rust/native/macos_bridge.m` | `AXUIElement`, `ScreenCaptureKit`, Apple `Vision` OCR, `ApexUseCursorView` overlay, `CGEvent`/`NSEvent`. **Live verified on macOS.** |
| **Windows 10/11** | `WindowsDriver` (`rust/src/driver/windows.rs`) | Pure-Rust Win32 FFI (`IUIAutomation` COM, `PrintWindow`/`BitBlt` capture, WinRT `Windows.Media.Ocr`, `SendInput`, layered `AgentCursor`, `som_png` overlay). **Cross-checked for `x86_64-pc-windows-gnu`; live Windows host check pending.** |
| **Linux (X11 / Wayland)** | `LinuxDriver` (`rust/src/driver/linux.rs`) | Runtime `dlopen` `libX11` / `libXtst` + `AT-SPI2` (`gi.repository.Atspi`), Wayland capture/input helpers (`grim`/`spectacle`/`gnome-screenshot`, `wtype`/`ydotool`), `tesseract` OCR, `som_png` overlay. **Cross-checked for `x86_64-unknown-linux-gnu`; live Linux host check pending.** |

---

## Quick Start

### 1. Build the release binary

```sh
cargo build --release --manifest-path rust/Cargo.toml
```

This compiles `rust/target/release/apexuse-mcp` standalone with zero external Rust crate dependencies (and builds `libapexuse_macos.a` on macOS).

### 2. Platform setup & permissions check

```sh
./rust/target/release/apexuse-mcp doctor
```

- **macOS:** Enable **Accessibility** and **Screen Recording** for the terminal/IDE launching `apexuse-mcp` in **System Settings → Privacy & Security**.
- **Windows:** No build dependencies needed beyond Rust; runs against built-in Win32 / UIA / WinRT APIs.
- **Linux:** Runs natively on X11 via `libX11`/`libXtst` (`dlopen`). For full semantic accessibility, OCR, and Wayland support install your distro packages as needed (e.g., `python3-gi`, `gir1.2-atspi-2.0`, `tesseract-ocr`, `xdotool`, or `grim` / `wtype` / `ydotool` on Wayland).

### 3. Generate MCP client config

```sh
./rust/target/release/apexuse-mcp install antigravity
# Also supported: gemini | claude | codex
```

See [agent-configs/README.md](./agent-configs/README.md) for full configuration snippets and skill installation details (`.agents/skills/apexuse/SKILL.md`).

---

## MCP Tools Reference

| Tool | Primary Actions / Parameters |
| --- | --- |
| `inspect_app` | Read-only Accessibility snapshot for `app` (bundle ID, friendly app name, or `"desktop"`), optional `window` index and `query` filter. |
| `visual_inspect` | ScreenCaptureKit + Accessibility + Vision OCR SoM annotation (`app`, optional `window`, `ocr="auto"\|"fast"\|"accurate"\|"off"`, `max_marks` `1..=150`). |
| `computer` | `inspect`, `click`, `double_click`, `right_click`, `hover` (`ref`/`mark`/`x`,`y`), `drag`, `type`, `press_key`, `scroll` (optional `ref`/`mark`/`x`,`y`), `run_actions`, `wait_for`, `confirm`. |
| `desktop` | `list_applications`, `launch_application`, `activate_application`, `open_url`, `visual_click`, `double_click`, `right_click`, `hover`, `drag`. |
| `screenshot` | Unannotated PNG capture of the largest window of a permitted running app after safety preflight. |
| `workflow_memory` | `list`, `lookup`, `record`, `replay` (with optional `step_args`), `clear`. |

---

## Configuration & Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `APEXUSE_POLICY` | `./policy.yaml` (then ancestor search) | Path to YAML policy file (`policy.yaml` or `policy.autonomous.yaml`). |
| `APEXUSE_STATE_DIR` | `~/.apexuse` | Directory for `logs/audit.jsonl`, `memory/recipes.json`, and `STOP`. |
| `APEXUSE_MOCK` | `0` | Set to `1` (or pass `--mock`) to run with the in-memory `MockDriver`. |

*(Backwards-compatible fallbacks `HANDSMCP_POLICY`, `HANDSMCP_STATE_DIR`, and `HANDSMCP_MOCK` are also honored.)*

Create `~/.apexuse/STOP` at any time to immediately halt all desktop mutations (`panic_stop`). Delete the file to resume.

---

## Testing

Run Rust unit & integration tests:

```sh
cargo test --manifest-path rust/Cargo.toml
```

Run the Python end-to-end stdio MCP client suite against the built release binary (requires `.venv` with `mcp` and `pytest`):

```sh
.venv/bin/pytest -q
```

---

## Layout

```text
├── .agents/skills/apexuse/SKILL.md  # Agent usage skill instructions
├── agent-configs/README.md          # Client setup (Antigravity, Gemini CLI, Codex, Claude Code)
├── policy.yaml                      # Default policy
├── policy.autonomous.yaml           # Explicit autonomous policy preset
├── pyproject.toml                   # Python E2E test harness dependencies
├── rust/
│   ├── Cargo.toml                   # apexuse-mcp crate (no external crate deps)
│   ├── build.rs                     # Compiles native Objective-C bridge on macOS
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
