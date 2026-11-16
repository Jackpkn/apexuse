sta# ApexUse Universal Agent Wiring & Speed Overhaul — Design Spec

**Date:** 2026-11-17  
**Status:** Ready for Approval (Builder Check Completed)  
**Target Version:** `0.3.0`  
**Selected Approach:** Option A — Universal Agent Installer (`npx` + `--write` multi-agent CLI) + Core Speed & Token-Efficiency Engine

---

## 1. Intent & The Why

ApexUse (`apexuse-mcp`) already combines hybrid OS Accessibility + Set-of-Mark (`[m1]..[mN]`) visual grounding, local OCR, batched `run_actions`, safety redaction, and workflow memory in a zero-dependency Rust binary. To make it effortlessly adoptable and fast across real-world coding workflows, two areas need upgrading together:

1. **Universal Agent Wiring:** Any developer on macOS, Windows, or Linux—with or without Rust/Cargo installed—must be able to install `apexuse-mcp` and wire both the MCP server and its bundled skill into **any major coding agent** in one command without hand-editing JSON or TOML configs.
2. **Maximum Speed & Low Token Cost:** Every interactive step must minimize OS execution latency, capture/encode overhead, VLM image token size, and wasted LLM round-trips without weakening ApexUse's native security guardrails.

---

## 2. Goals, Non-Goals, and Anti-Goals

### Goals
* **One-command universal installation:** Support `npx -y apexuse-mcp` / `npm i -g apexuse-mcp` backed by SHA-256-verified GitHub Release binaries (adding `macos-x86_64` alongside `macos-arm64`, `linux-x86_64`, `windows-x86_64`), alongside `crates.io` (`cargo install apexuse-mcp`) and source builds.
* **Safe config auto-wiring (`install`):** Upgrade `apexuse-mcp install` to merge configs non-destructively across **Antigravity, Claude Code, Claude Desktop, OpenAI Codex CLI/App, Gemini CLI, Cursor, Windsurf, VS Code, and OpenCode**, plus `--with-skill`.
* **Direct CLI automation & diagnostics:** Support `apexuse-mcp call` (single call or multi-step batch `--calls` / `--calls-file` sharing snapshot refs/marks in one process) and `apexuse-mcp config` for persistent runtime defaults (`~/.apexuse/config.json`).
* **Instant UI manipulation (`set_value`, `select_text`, fast typing):** Replace slow 10ms/character keystroke loops with native AX/UIA/AT-SPI value & selection APIs and chunked/fast fallback typing.
* **Smaller, faster screenshots:** Add pure-Rust baseline JPEG encoding (`0` external Rust crates maintained), configurable resolution caps, adaptive early-exit change verification, and macOS window/capture caching.
* **Fewer LLM round-trips:** Add inline `set_value` / `select_text` / `wait_for` inside `run_actions`, raise step limit (`12` -> `24`), add deterministic semantic targeting (`target: { query, role }`), and preserve form snapshots across non-navigating field edits.

### Non-Goals
* Building a Chrome/Dia browser extension or custom CDP automation layer.
* Implementing a macOS screen-unlock daemon (`Locked Use`).
* Automating `npm publish` inside GitHub Actions (keep manual `npm publish` alongside `cargo publish` so tag releases never break without an `NPM_TOKEN` secret).
* Adding external Rust crate dependencies to `rust/Cargo.toml`.

### Anti-Goals
* **Never clobber existing agent configs:** Back up non-empty config files (`.bak`) before atomic temp-file writes, preserve unrelated MCP servers, and abort cleanly on invalid JSON/comments.
* **Never bypass safety boundaries for speed:** Protected-app checks, password/secure-field redaction (`check_hit_safe`, `verify_point_safe`, AT-SPI `PASSWORD_TEXT`), clipboard paste blocking, and `~/.apexuse/STOP` remain enforced on `set_value`, `select_text`, semantic targeting, reused snapshots, and CLI `call`.
* **Never add cloud relays or external LLM API keys.**

---

## 3. Constraints

* **Zero external Rust crate dependencies:** `rust/Cargo.toml` stays dependency-free.
* **Cross-OS build & CI parity:** Must compile and pass `cargo test`, `doctor`, and `pytest -q` (`--mock`) on macOS (`arm64` & `x86_64`), Linux (`ubuntu-latest`), and Windows (`windows-latest`), including updating `rust/build.rs` to pass explicit `-arch` (`arm64` / `x86_64`) flags to `clang`.
* **Version synchronization (`0.3.0`):** `rust/Cargo.toml`, `rust/Cargo.lock`, `rust/src/mcp.rs`, `pyproject.toml`, `server.json`, and `npm/apexuse-mcp/package.json` all advance to `0.3.0`.
* **Backward compatibility:** Existing MCP tool names stay intact; bare `install <agent>` without `--write` continues printing snippets to stdout; `HANDSMCP_*` env fallbacks remain supported.

---

## 4. Approaches Considered

1. **Option A — Universal Agent Installer + Speed-Overhauled Engine *(Chosen)***
   * Ships `npm/apexuse-mcp` launcher + `--write` multi-agent config merging + `call`/`config` CLI alongside native AX `set_value`/`select_text`, pure-Rust JPEG default + scale presets, capture caching, adaptive verify polling, and semantic/expanded batching.
2. **Option B — Core Speed Only** *(Rejected: leaves config setup manual and lacks `npx` distribution.)*
3. **Option C — Full Ecosystem Expansion (+ Browser Extension + macOS Locked Use)** *(Rejected: high multi-runtime maintenance and OS privilege risk.)*

---

## 5. Architecture & Detailed Design

### Part 1: Universal Agent Wiring (`npm/` + `rust/src/main.rs` + Release Pipeline)

#### 1.1 Zero-Dependency `npm` Launcher (`npm/apexuse-mcp/`)
* **Files:** `package.json`, `bin/apexuse-mcp.js`, `README.md`, `LICENSE`.
  * `"name": "apexuse-mcp"`, `"version": "0.3.0"`, `"dependencies": {}`, `"engines": { "node": ">=18" }`.
  * `"bin": { "apexuse-mcp": "bin/apexuse-mcp.js", "apexuse": "bin/apexuse-mcp.js" }`.
* **Target resolution & checksums:**
  * Maps `(process.platform, process.arch)` -> `macos-arm64` (`tar.gz`), `macos-x86_64` (`tar.gz`), `linux-x86_64` (`tar.gz`), `windows-x86_64` (`zip`).
  * Binary lookup order: `APEXUSE_BIN` env override -> `~/.apexuse/bin/v0.3.0/apexuse-mcp(.exe)` cache -> system `apexuse-mcp` on `PATH` only if `--use-system` is passed (wrapper strips `--use-system` before spawning binary so Rust CLI never sees an unknown flag) -> download archive + `.sha256` from GitHub Releases `v0.3.0`, verify leading hex token of `.sha256`, extract safely, chmod `+x`, exec with inherited `stdio`.
  * All download progress/diagnostics go to `stderr` so `stdout` stays clean for MCP stdio JSON-RPC.
* **Cross-arch macOS release builds (`rust/build.rs` + `.github/workflows/release.yml`):**
  * Update `rust/build.rs` to inspect `CARGO_CFG_TARGET_ARCH` and pass `-arch x86_64` or `-arch arm64` to `clang`.
  * Add `macos-x86_64` matrix entry in `.github/workflows/release.yml` building with `rustup target add x86_64-apple-darwin` + `--target x86_64-apple-darwin` on `macos-latest`.
  * Keep `npm publish` manual (same workflow as `crates.io` / `mcp-publisher`).

#### 1.2 Multi-Agent `install` CLI (`rust/src/installer.rs`, wired from `rust/src/main.rs`)
* **Command:** `apexuse-mcp install <agent|all> [--write] [--dry-run] [--scope user|project] [--npx] [--policy <path>] [--with-skill]`
* **Targets & formats:**
  * `antigravity`: `~/.gemini/antigravity/mcp_config.json` / `.agents/mcp_config.json` (`"mcpServers"`).
  * `gemini`: `~/.gemini/settings.json` / `.gemini/settings.json` (`"mcpServers"`).
  * `claude`: `~/.claude.json` / `.mcp.json` (`"mcpServers"`; snippet mode preserves `claude mcp add`).
  * `claude-desktop`: macOS `~/Library/Application Support/Claude/claude_desktop_config.json`, Windows `%APPDATA%\Claude\claude_desktop_config.json`, Linux `~/.config/Claude/claude_desktop_config.json` (`"mcpServers"`).
  * `codex`: `~/.codex/config.toml` / `.codex/config.toml` (`[mcp_servers.apexuse]` + `[mcp_servers.apexuse.env]` TOML table upsert preserving other servers and top-level keys).
  * `cursor`: `~/.cursor/mcp.json` / `.cursor/mcp.json` (`"mcpServers"`).
  * `windsurf`: `~/.codeium/windsurf/mcp_config.json` (`"mcpServers"`).
  * `vscode`: project `.vscode/mcp.json` or OS user profile `Code/User/mcp.json` (`"servers"`).
  * `opencode`: `~/.config/opencode/opencode.json` / `opencode.json` (`"mcp"` with `"apexuse": { "type": "local", "command": [...], "environment": {...}, "enabled": true }`).
  * `all` (requires `--write` or `--dry-run`): detects installed agent config roots/CLIs, wires detected agents cleanly, and reports skipped/uninstalled ones.
* **Merge safety & extras:**
  * Bare `install <agent>` (no `--write`) prints the snippet to stdout unchanged.
  * `--write` backs up non-empty targets to `<file>.bak`, parses JSON cleanly (aborting with exit code `1` on invalid JSON rather than overwriting), and writes via temp-file + atomic rename.
  * `--npx` writes `"command": "npx"`, `"args": ["-y", "apexuse-mcp"]` (or OpenCode `"command": ["npx", "-y", "apexuse-mcp"]`).
  * `--with-skill` writes embedded `.agents/skills/apexuse/SKILL.md` (`include_str!`) into standard agent skill paths (`~/.agents/skills/apexuse/SKILL.md`, `~/.claude/skills/apexuse/SKILL.md`, `~/.codex/skills/apexuse/SKILL.md`, etc.).

#### 1.3 Persistent Config (`~/.apexuse/config.json`) & CLI `call` / `doctor`
* **Config loader (`rust/src/config.rs`):** Loads `$APEXUSE_STATE_DIR/config.json` (overrideable via `APEXUSE_CONFIG`) with compiled fast defaults:
  ```json
  {
    "image": {
      "format": "jpeg",
      "quality": 80,
      "max_width": 1280
    },
    "timing": {
      "settle_ms": 80,
      "poll_interval_ms": 30,
      "key_delay_us": 500
    },
    "snapshot": {
      "reuse_form_refs": true
    }
  }
  ```
* **CLI commands:**
  * `apexuse-mcp --version` / `-V`: prints `apexuse-mcp 0.3.0`.
  * `apexuse-mcp config [show|get <key>|set <key> <val>|reset]`.
  * `apexuse-mcp call <tool> [--args '<json>'] [--save-image <path>]` and `apexuse-mcp call --calls '<json-array>' | --calls-file <path> [--sleep-ms <n>]` executing sequentially inside one `Gateway` instance so `e1..eN` / `[m1]..[mN]` carry across steps.
  * `apexuse-mcp doctor [--check]`: prints existing JSON stdout; `--check` exits non-zero when permissions fail.

---

### Part 2: Engine Speed & Token-Efficiency Overhaul

#### 2.1 Direct Native `set_value` & `select_text` Actions
* **Gateway & MCP schema (`rust/src/mcp.rs`, `rust/src/gateway.rs`, `rust/src/driver/mod.rs`):**
  * Add `ActionRequest::SetValue { bundle_id, x, y, text }` and `ActionRequest::SelectText { bundle_id, x, y, start: Option<usize>, end: Option<usize> }`.
  * Add `action = "set_value"` (`ref`/`mark`/`x,y`/`target` + `text`) and `action = "select_text"` (`ref`/`mark`/`x,y`/`target` + optional `start`/`end`) to `computer` and `run_actions`.
  * **Value rules & fail-closed safety boundary *(explicit call)*:**
    * Allow `text: ""` (empty string) on `set_value` to clear fields cleanly; keep rejecting empty `text` on `type`.
    * `set_value` **fails closed (`target_identity_unverified`)** if OS Accessibility cannot verify the target is an editable control (e.g. raw `(x, y)` coordinates or OCR-only `[mN]` marks that miss AX editable widgets). Only verified editable controls (including Chromium/Electron elements where Accessibility resolves an editable node/attribute or `contenteditable` root) run direct value write or focused `Cmd+A`/`Ctrl+A` fallback.
    * Password/protected-app/control-char/rate-limit/audit/STOP checks match `type`. `workflow_memory` treats `set_value` as parameterized like `type` (never storing text values).
* **Driver behavior:**
  * **macOS (`macos_bridge.m`):** `set_value` verifies `check_hit_safe` + `resolve_editable_element`, sets `kAXFocusedAttribute = true`, writes `kAXValueAttribute` directly (`ax_set_value`), and falls back on Chromium/web rejections to focused `Cmd+A` + fast chunked Unicode input (`pid_set_value_fallback`). `select_text` sets `kAXSelectedTextRangeAttribute` (`CFRange`), or `[0, len]` / focused `Cmd+A` when omitted.
  * **Windows (`windows.rs`):** `set_value` checks `verify_point_safe` -> UIA `ValuePattern::SetValue` (`windows_uia_value`) -> verified editable fallback `Ctrl+A` + fast `SendInput`; `select_text` uses UIA `TextPattern` / verified `Ctrl+A`.
  * **Linux (`linux.rs`):** `set_value` uses AT-SPI `EditableText::setTextContents` (checking `PASSWORD_TEXT` first) -> verified `Ctrl+A` + fast typing; `select_text` uses AT-SPI `EditableText::selectText` / `Text::setSelection` -> `Ctrl+A`.
* **Fast fallback typing (`type`):**
  * **macOS:** Update `type_unicode_text_to_pid` to send multi-character UTF-16 slices (up to 16 code units per `CGEventKeyboardSetUnicodeString` pair for printable text, single key events for `\n`/`\r`/`\t`) with configurable `timing.key_delay_us` (default `500`µs instead of `10000`µs/char).
  * **Windows & Linux:** Batch `SendInput` events per call (Windows) and use `--delay 0` / single `x_sync` flush (Linux).

#### 2.2 Pure-Rust JPEG Encoding (Default), Scaling & Capture Caching
* **Zero-dep Pure-Rust Baseline JPEG Encoder (`rust/src/som_png.rs` or `rust/src/image_codec.rs`):**
  * Encodes RGBA buffers directly to baseline JPEG (YCbCr, 8x8 FDCT, standard quantization & Huffman tables) with **0 external crates**.
  * **Default format *(explicit call)*:** Default runtime `image.format` is **`"jpeg"` (`quality = 80`, `max_width = 1280`)** on `visual_inspect` and `screenshot` for immediate latency/token wins, while explicit `format = "png"` remains supported in tool args and `~/.apexuse/config.json`.
  * Generalize `ToolOutput` (`image_b64`, `image_mime_type`) so MCP `ImageContent` returns `"image/jpeg"` or `"image/png"` accurately, plus `image_format`, `image_width`, `image_height`, `image_bytes` in `structuredContent`.
  * Update `MockDriver` / `Gateway` image encoding path and `tests/test_rust_mcp.py` (plus Rust unit tests) to assert JPEG under defaults and PNG under explicit `format: "png"`.
* **macOS capture fast path (`macos_bridge.m`):**
  * Downscale directly via `SCStreamConfiguration.width/height` when `max_width > 0`, avoiding oversized Retina captures.
  * Add short-TTL (`1500ms`) resolved window cache keyed by `(pid, window_index, isDesktop)` with automatic cache bypass/invalidation on capture miss so consecutive calls skip `SCShareableContent` enumeration.

#### 2.3 Fewer Round-Trips & Adaptive Verification (`rust/src/gateway.rs`)
* **Adaptive early-exit `verify_change`:**
  * Poll every `poll_interval_ms` (default `30ms`) up to `settle_ms` (default `80ms` unless overridden by caller) and return as soon as `ax_changed || visual_changed`.
* **Semantic targeting (`target: { query, role }`):**
  * Supported on `computer` (`click`, `double_click`, `right_click`, `hover`, `type`, `set_value`, `select_text`) and `run_actions` steps.
  * **Lookup lifecycle *(explicit call)*:** Reuse cached unfiltered `elements_by_ref` if active for the target app (`0ms`); automatically call `driver.inspect_app` once if no unfiltered snapshot matches that app, if `fresh: true` is set, or if cached lookup returns 0 matches (single refresh).
  * **Disambiguation *(explicit call)*:** Match priority mirrors `find_recipe_element`: exact case-insensitive `label` match (respecting optional `role`) beats partial substring match (`"Save"` beats `"Save As..."`); fail closed with `target_not_found` (`0` matches) or `ambiguous_target` (`> 1` exact matches, or `0` exact + `> 1` partial matches).
* **Expanded `run_actions`:**
  * Raise `steps` cap `12` -> `24`.
  * Support `set_value`, `select_text`, `wait_for`, and `target: { query, role }` steps inside `run_actions`, refreshing internal step snapshot maps after inline `wait_for` or `inspect` steps.
* **Lightweight form-safe snapshot reuse (`keep_snapshot` / `config.snapshot.reuse_form_refs`):**
  * **Explicit call:** `config.snapshot.reuse_form_refs` defaults to **`true`**, and per-action `keep_snapshot` overrides it cleanly.
  * Add fast O(1) `OsDriver::window_state(&self, bundle_id: &str) -> Result<WindowState, DriverError>` returning `{ frontmost_bundle_id, pid, window_count, active_window_frame }` (`<2ms`, **no** full AX tree walk and **no** Python AT-SPI subprocess on Linux).
  * After non-navigating value edits (`type`, `set_value`, `select_text`), if reuse is enabled and `window_state` shows `frontmost_bundle_id`, target `pid`, `window_count`, and active window frame match the snapshot (allowing background-app edits when focus hasn't shifted), preserve **both** `elements_by_ref` and `marks_by_id` (along with `epoch`, consuming only single-use confirmation tokens). Any click, key press, drag, scroll, activate, or window change invalidates immediately.

---

## 6. Builder Calls Made & Open Check Items

Summary of engineering calls resolved cleanly from the reviewer check (already reflected above):
1. **Default `image.format = "jpeg"` (`quality = 80`, `max_width = 1280`)** with full `format = "png"` opt-in and test updates.
2. **`set_value` allows `text: ""` (clear field) and fails closed** on unverified `(x, y)` / OCR-only targets.
3. **Version `0.3.0`** synced across Rust, Python, MCP Registry, and npm launcher; `rust/build.rs` updated for `-arch` cross-compiles; manual `npm publish`.
4. **`keep_snapshot` follows `config.snapshot.reuse_form_refs` (`true` default)**, uses O(1) `<2ms` `OsDriver::window_state` verification, and preserves both `elements_by_ref` and `marks_by_id`.
5. **Semantic `target` reuses active unfiltered snapshots first** (single fallback re-inspect on miss) and prioritizes exact label match over partial substring match.
