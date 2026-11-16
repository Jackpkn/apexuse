# ApexUse Universal Agent Wiring & Speed Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver `apexuse-mcp` v0.3.0 with universal one-command coding-agent setup (`npx` launcher + non-destructive `--write` installer + `call`/`config` CLI) and a fast execution/vision engine (native `set_value`/`select_text`, chunked fast typing, pure-Rust JPEG default + scale presets, macOS capture caching, adaptive `verify_change`, semantic targeting, and `window_state`-verified form ref reuse).

**Architecture:** Extend `OsDriver` with lightweight `window_state`, `SetValue`, `SelectText`, and `ImageEncodeOptions`, backed by a persistent `~/.apexuse/config.json` store and a zero-dep pure-Rust JPEG encoder inside `som_png.rs`. Expose the speed upgrades through `Gateway` and `mcp.rs`, wire a multi-agent JSON/TOML config installer and CLI `call` runner in `main.rs`, and ship a dependency-free Node `>=18` `npx` wrapper backed by SHA-256-verified GitHub Release archives (adding `macos-x86_64`).

**Tech Stack:** Rust (`std` + OS C ABI/COM/`dlopen`, **0 external crates**), Objective-C/AppKit/AX/ScreenCaptureKit/Vision (`macos_bridge.m`), Win32/UIA/WinRT (`windows.rs`), X11/AT-SPI2/Wayland (`linux.rs`), Node.js `>=18` (`npm/apexuse-mcp`), Python `pytest` + `mcp` E2E suite.

**Spec:** [`docs/superpowers/specs/2026-11-17-universal-wiring-and-speed-design.md`](../specs/2026-11-17-universal-wiring-and-speed-design.md)

## Global Constraints

- Zero external Rust crate dependencies (`rust/Cargo.toml` stays completely dependency-free).
- Zero external Node.js runtime dependencies (`npm/apexuse-mcp/package.json` `"dependencies": {}`, `"engines": { "node": ">=18" }`).
- Cross-OS CI parity on macOS (`arm64` + `x86_64`), Linux (`ubuntu-latest`), and Windows (`windows-latest`), including `-arch` cross-compilation in `rust/build.rs`.
- Synchronized version `0.3.0` across `rust/Cargo.toml`, `rust/Cargo.lock`, `rust/src/mcp.rs`, `pyproject.toml`, `server.json`, and `npm/apexuse-mcp/package.json`.
- Safety boundaries invariant: protected-app checks, password/secure-field redaction, clipboard paste blocking, and `~/.apexuse/STOP` kill switch always execute on `set_value`, `select_text`, semantic `target`, reused form snapshots, and CLI `call`.
- Non-destructive defaults: bare `install <agent>` without `--write` prints to stdout; `--write` writes `<file>.bak` before atomic temp-file rename and aborts on invalid config syntax.
- `stdout` stays strictly reserved for JSON-RPC / CLI JSON output; launcher downloads log only to `stderr`.

## Review Focus

- **Unparseable or commented JSON config passed to `install --write`:** Must abort cleanly with non-zero exit and leave the original file untouched rather than truncating or overwriting.
- **Updating `[mcp_servers.apexuse]` in `~/.codex/config.toml` alongside sibling `[mcp_servers.*]` tables and top-level keys:** Must replace existing `apexuse` sections cleanly while preserving all unrelated TOML sections and comments.
- **Wrapper `--use-system` flag & mixed `.sha256` formats (`sha256sum` / `shasum` / PowerShell):** Launcher must strip `--use-system` before spawning `apexuse-mcp` and parse leading 64-char hex digests regardless of whitespace/filename format.
- **JPEG encoding on non-8-aligned capture dimensions (e.g. `123x77`):** Pure-Rust 8x8 MCU edge padding must not panic or distort bottom/right pixels.
- **Focus shift mid-form under `reuse_form_refs` / `confirmation: required`:** Changing `frontmost_bundle_id` (or target window count/frame) between form steps must invalidate `elements_by_ref` and `marks_by_id` (`stale_ref`), while background-app form edits without focus shift preserve both and still expire consumed single-use confirmation tokens.

---

### Task 1: Runtime Config Store (`rust/src/config.rs`) & Version `0.3.0` Bump

**Files:**
- Create: `rust/src/config.rs`
- Modify: `rust/src/lib.rs`
- Modify: `rust/Cargo.toml:1-10`, `rust/Cargo.lock:5-8`, `rust/src/mcp.rs:220-226`, `pyproject.toml:1-6`, `server.json:5-18`

**Interfaces:**
- Produces in `rust/src/config.rs`:
  - `pub enum ImageFormat { Jpeg, Png }` (`as_str(&self) -> &'static str`, `mime_type(&self) -> &'static str`)
  - `pub struct ImageConfig { pub format: ImageFormat, pub quality: u8, pub max_width: usize }`
  - `pub struct TimingConfig { pub settle_ms: u64, pub poll_interval_ms: u64, pub key_delay_us: u64 }`
  - `pub struct SnapshotConfig { pub reuse_form_refs: bool }`
  - `pub struct AppConfig { pub image: ImageConfig, pub timing: TimingConfig, pub snapshot: SnapshotConfig }`
  - `AppConfig::load_or_default(path: &Path) -> Result<Self, String>`
  - `AppConfig::save(&self, path: &Path) -> Result<(), String>`
  - `AppConfig::to_json(&self) -> JsonValue`
  - `AppConfig::get_key(&self, key: &str) -> Result<JsonValue, String>`
  - `AppConfig::set_key(&mut self, key: &str, raw_value: &str) -> Result<(), String>`
  - `pub fn resolve_config_path(state_dir: &Path) -> PathBuf` (respects `APEXUSE_CONFIG` env override)

- [ ] **Step 1: Write unit tests for `AppConfig` defaults, persistence, dot-key updates, and validation bounds**

Add `#[cfg(test)] mod tests` in `rust/src/config.rs` asserting:
- `AppConfig::default()` yields `image.format == ImageFormat::Jpeg`, `quality == 80`, `max_width == 1280`, `timing.settle_ms == 80`, `poll_interval_ms == 30`, `key_delay_us == 500`, `snapshot.reuse_form_refs == true`.
- `load_or_default` returns defaults when file does not exist and rejects malformed JSON.
- `set_key` accepts `"image.format" = "png"`, `"image.quality" = "90"`, `"image.max_width" = "1600"`, `"timing.settle_ms" = "120"`, `"timing.poll_interval_ms" = "25"`, `"timing.key_delay_us" = "0"`, `"snapshot.reuse_form_refs" = "false"`, and rejects out-of-range values (`quality` outside `1..=100`, `max_width` outside `640..=2560`, `settle_ms` outside `10..=3000`, `poll_interval_ms` outside `10..=500`, `key_delay_us` outside `0..=50000`, unknown keys).
- Roundtrip `save` -> `load_or_default` preserves all fields.

- [ ] **Step 2: Run tests to verify failure before implementation**

Run: `cargo test --manifest-path rust/Cargo.toml config::tests`
Expected: FAIL (`module config not found`)

- [ ] **Step 3: Implement `rust/src/config.rs` and export `pub mod config;` in `rust/src/lib.rs`**

Parse JSON via `crate::json::parse_json` applying partial field overrides onto `Default::default()`, atomic write via sibling `.tmp` file + rename in `save`, and `resolve_config_path` checking non-empty `APEXUSE_CONFIG` before `state_dir.join("config.json")`.

- [ ] **Step 4: Bump project version `0.2.1` -> `0.3.0`**

Update `version` to `"0.3.0"` in `rust/Cargo.toml`, `rust/Cargo.lock`, `rust/src/mcp.rs` (`serverInfo.version`), `pyproject.toml`, and `server.json`. Verify `cargo test --manifest-path rust/Cargo.toml` passes (note: update any exact `"0.2.1"` assertions if added anywhere).

- [ ] **Step 5: Run tests and verify pass**

Run: `cargo test --manifest-path rust/Cargo.toml`
Expected: PASS (`config::tests` and all existing unit tests green)

- [ ] **Step 6: Commit**

```bash
git add rust/src/config.rs rust/src/lib.rs rust/Cargo.toml rust/Cargo.lock rust/src/mcp.rs pyproject.toml server.json
git commit -m "feat(config): add persistent AppConfig store and bump workspace to v0.3.0"
```

---

### Task 2: Pure-Rust Baseline JPEG Encoder & Generalized Image Encoding (`rust/src/som_png.rs`)

**Files:**
- Modify: `rust/src/som_png.rs:746-928`

**Interfaces:**
- Consumes: `crate::config::ImageFormat` (or explicit `(ImageFormat, u8)` quality parameter)
- Produces in `rust/src/som_png.rs`:
  - `pub fn encode_rgba_jpeg_bytes(w: usize, h: usize, rgba: &[u8], quality: u8) -> Vec<u8>`
  - `pub fn encode_rgba_jpeg_b64(w: usize, h: usize, rgba: &[u8], quality: u8) -> String`
  - `pub fn encode_rgba_image_b64(w: usize, h: usize, rgba: &[u8], format: ImageFormat, quality: u8) -> String`
  - `pub fn render_som_image_to_b64(img_or_cap_id: &str, marks_json: &str, win_frame: &Rect, format: ImageFormat, quality: u8, max_width: usize) -> Result<String, DriverError>`
  - Keep `pub fn render_som_rgba_to_b64(png_b64_or_cap_id: &str, marks_json: &str, win_frame: &Rect) -> Result<String, DriverError>` delegating to PNG at `max_width = 1440` for backward-compatible unit callers.

- [ ] **Step 1: Write failing unit tests for JPEG encoding, non-8-aligned edge padding, and SoM JPEG output**

Add tests in `rust/src/som_png.rs`:
- `jpeg_encoder_writes_valid_baseline_jfif_for_non_multiple_of_eight_dims`: encodes `123x77` synthetic RGBA gradient at `quality=80` and `quality=95`, asserts bytes start with `0xFF, 0xD8, 0xFF, 0xE0` (SOI + APP0 JFIF), contain `SOF0` (`0xFF, 0xC0`) with big-endian height `77` and width `123`, end with `0xFF, 0xD9` (EOI), and `q95.len() > q80.len()`.
- `render_som_image_supports_jpeg_and_custom_max_width`: caches a `1600x900` frame, calls `render_som_image_to_b64(&cap_id, marks_json, &win, ImageFormat::Jpeg, 80, 1280)`, asserts base64 decodes to JPEG (`starts_with(&[0xFF, 0xD8, 0xFF])`) and parses `SOF0` dimensions `(1280, 720)`.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path rust/Cargo.toml som_png::tests`
Expected: FAIL (`encode_rgba_jpeg_bytes` / `render_som_image_to_b64` not found)

- [ ] **Step 3: Implement zero-dep pure-Rust baseline JPEG encoder & generalized `render_som_image_to_b64` in `rust/src/som_png.rs`**

Implement standard ITU-T.81 baseline JPEG (JFIF APP0, Annex K standard Y/CbCr quantization tables scaled by `quality.clamp(1, 100)`, `SOF0` 4:4:4 or 4:2:0 — 4:4:4 keeps crisp text/SoM badge edges with negligible complexity, Annex K DC/AC Huffman tables, byte stuffing `0xFF -> 0xFF 0x00`, EOI). Update `render_som_image_to_b64` to accept either `cap_` cache IDs or PNG/RGBA inputs, apply `downscale_rgba` when `src_w > max_width.clamp(640, 2560)`, draw SoM overlays, and encode via `encode_rgba_image_b64`. Also check `macos_bridge.m` JPEG/PNG compatibility (Task 3 covers macOS direct JPEG capture/SoM).

- [ ] **Step 4: Run tests to verify pass (plus optional Python PIL sanity decode if available)**

Run: `cargo test --manifest-path rust/Cargo.toml som_png::tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add rust/src/som_png.rs
git commit -m "feat(codec): add zero-dependency pure-Rust baseline JPEG encoder and SoM scale control"
```

---

### Task 3: `OsDriver` Extensions & Native macOS Speed Upgrades (`driver/mod.rs`, `common.rs`, `macos*`, `build.rs`)

**Files:**
- Modify: `rust/build.rs:15-42`
- Modify: `rust/src/driver/mod.rs:177-295`
- Modify: `rust/src/driver/common.rs:1-60`
- Modify: `rust/native/macos_bridge.h:1-25`
- Modify: `rust/native/macos_bridge.m:918-1200,1687-1735,2100-2365`
- Modify: `rust/src/driver/macos.rs:1-514`

**Interfaces:**
- Consumes: `crate::config::ImageFormat`
- Produces in `rust/src/driver/mod.rs`:
  - `pub struct ImageEncodeOptions { pub format: ImageFormat, pub quality: u8, pub max_width: usize }`
  - `pub struct WindowState { pub frontmost_bundle_id: String, pub pid: i64, pub windows_total: usize, pub active_window_frame: Option<Rect> }`
  - `ActionRequest::SetValue { pub bundle_id: String, pub x: f64, pub y: f64, pub text: String }`
  - `ActionRequest::SelectText { pub bundle_id: String, pub x: f64, pub y: f64, pub start: Option<usize>, pub end: Option<usize> }`
  - Update `CaptureResult` to track `pub image_format: ImageFormat` and generalize `image_base64: String` (keep helper or rename cleanly across callers).
  - Extend `OsDriver`:
    - `fn window_state(&self, bundle_id: &str) -> Result<WindowState, DriverError>` (default fallback `Unsupported` or derived for mocks)
    - Update `capture_and_ocr` / `render_som_image` signatures (or pass `&ImageEncodeOptions` cleanly so `max_width` downscale + `format` + `quality` reach native drivers, while keeping `encode_image: bool` for OCR-only capture).
    - Add configurable `key_delay_us: u64` on `ActionRequest::TypePoint` (or `key_delay_us` field on `TypePoint`) so `AppConfig::timing.key_delay_us` flows into macOS/Win/Linux typing.
- Produces in `rust/src/driver/common.rs`:
  - `pub fn is_editable_role(role: &str) -> bool` (true for `TextField`, `TextArea`, `SearchField`, `ComboBox`)

- [ ] **Step 1: Add `-arch` flag detection to `rust/build.rs` and `OsDriver` / `WindowState` / `SetValue` / `SelectText` unit tests**

- In `rust/build.rs`, read `CARGO_CFG_TARGET_ARCH` (`"x86_64"` -> `["-arch", "x86_64"]`, `"aarch64"` -> `["-arch", "arm64"]`) and pass both to `clang` and `println!("cargo:rustc-link-arg=-arch")` when needed.
- Add unit tests in `rust/src/driver/mod.rs` and `rust/src/driver/macos.rs` covering `is_editable_role`, `WindowState`, and `MacosDriver::render_som_image` with `ImageFormat::Jpeg` (`starts_with("/9j/")` base64 JPEG header).

- [ ] **Step 2: Run unit tests to verify failure before implementation**

Run: `cargo test --manifest-path rust/Cargo.toml driver::`
Expected: FAIL (missing `SetValue`, `SelectText`, `WindowState`, `render_som_image`)

- [ ] **Step 3: Update `rust/src/driver/mod.rs`, `rust/src/driver/common.rs`, `macos_bridge.{h,m}`, and `rust/src/driver/macos.rs`**

Key macOS bridge behaviors in `rust/native/macos_bridge.m`:
1. **`apexuse_macos_window_state(const char *bundle_id)`:** Fast query (<2ms) resolving `NSWorkspace.sharedWorkspace.frontmostApplication.bundleIdentifier`, target `pid`, `AXUIElementCopyAttributeValue(appEl, kAXWindowsAttribute)` count (or `CGWindowListCopyWindowInfo` fallback), and focused/primary window frame without traversing child elements.
2. **`set_value` & `select_text` in `apexuse_macos_perform_action`:**
   - `set_value`: call `check_hit_safe` -> `resolve_editable_element`; if `!editable` return `ok: false, code: "target_identity_unverified"`. Set `kAXFocusedAttribute = true` and `kAXValueAttribute = text` (`method: "ax_set_value"`). If AX write fails AND `webTarget` (Chromium/Electron `isChrome || is_in_web_content`), click + `Cmd+A` + `delete` + fast `type_unicode_text_to_pid(eventPid, text, keyDelayUs)` (`method: "pid_set_value_fallback"`) — note when `text` is empty `""`, only `Cmd+A` + `delete` runs cleanly! Outside web content, fail closed (`replacement_set_failed`).
   - `select_text`: verify `check_hit_safe` -> `resolve_editable_element` (fail closed if `!editable`). Set `kAXFocusedAttribute = true`. Compute `CFRange` from `(start, end)` clamped to string length (or `[0, len]` when omitted) and apply `kAXSelectedTextRangeAttribute` (`method: "ax_select_text"`); on web content if `AXSelectedTextRange` fails and whole text requested, fall back to `Cmd+A` (`method: "pid_select_all"`).
3. **Chunked fast typing (`type_unicode_text_to_pid`):** Send printable contiguous spans (excluding `\n`, `\r`, `\t`) in batches of up to 16 UTF-16 `unichar`s per `CGEventKeyboardSetUnicodeString` down/up pair, sleeping `key_delay_us` (default `500`µs) between chunks instead of `10000`µs per character.
4. **Capture downscale, short-TTL window cache, and ImageIO JPEG output:**
   - Cache `(bundle_id, window_index, isDesktop) -> (pid, wid, winFrame, timestamp_ms)` for `1500ms`; verify the cached `CGWindowID` is still valid on capture, clearing cache and falling back immediately to `SCShareableContent` enumeration if `SCScreenshotManager` errors or `wid` vanishes.
   - In `SCStreamConfiguration`, downscale `cfg.width` / `cfg.height` proportionally when `max_width > 0` and `chosen.frame.size.width > max_width`.
   - Update `apexuse_macos_capture_and_ocr` and `apexuse_macos_render_som` to encode PNG or JPEG via `CGImageDestinationCreateWithData(..., kUTTypeJPEG / "public.jpeg" or "public.png", ...)` with `kCGImageDestinationLossyCompressionQuality` (`quality / 100.0`). Add minimal compilation stubs (`SetValue`, `SelectText`, `window_state`, updated `capture_and_ocr` / `render_som_image` / `TypePoint::key_delay_us` call sites) in `windows.rs`, `linux.rs`, and `gateway.rs` (`MockDriver` + handlers) so `cargo test` builds and passes cleanly at Task 3 before Task 4 & Task 5 flesh them out.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path rust/Cargo.toml`
Expected: PASS (keep `gateway.rs` compiling alongside Task 3 or split `gateway` test expectation updates into Task 5 cleanly — ensure every `ActionRequest::TypePoint` constructor & `OsDriver` impl matches at Task 3 commit!)

- [ ] **Step 5: Commit**

```bash
git add rust/build.rs rust/src/driver/ rust/native/
git commit -m "feat(macos,driver): add native set_value, select_text, chunked typing, and fast JPEG capture"
```

---

### Task 4: Windows & Linux Native `SetValue`, `SelectText`, Fast Typing, `WindowState`, and JPEG Encoding (`windows.rs`, `linux.rs`)

**Files:**
- Modify: `rust/src/driver/windows.rs:1343-2066`
- Modify: `rust/src/driver/linux.rs:1-120,1235-1765`

**Interfaces:**
- Consumes: `OsDriver`, `WindowState`, `ImageEncodeOptions`, `ActionRequest::{SetValue, SelectText, TypePoint}`, `crate::som_png::{encode_rgba_image_b64, render_som_image_to_b64}`
- Updates both `WindowsDriver` (`#[cfg(target_os = "windows")]` + non-Windows mock tests) and `LinuxDriver` (`#[cfg(target_os = "linux")]` + cross-OS unit tests).

- [ ] **Step 1: Write unit tests for Windows and Linux new driver request serialization / AT-SPI helper command args & X11 helper mappings**

Add tests in:
- `rust/src/driver/windows.rs`: test `WindowState` construction and verify `send_unicode_text_batched` builds multi-event `INPUT` slices cleanly.
- `rust/src/driver/linux.rs`: test AT-SPI Python helper embeds `set_value` (`EditableText.setTextContents` + `PASSWORD_TEXT` guard) and `select_text` (`EditableText` / `Text.setSelection`), plus verify JPEG/PNG dispatch path via `Mock` capture helper.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path rust/Cargo.toml driver::windows::tests driver::linux::tests`
Expected: FAIL on missing new helper behavior / assertions

- [ ] **Step 3: Implement `SetValue`, `SelectText`, `WindowState`, batched typing, and JPEG encoding in `windows.rs` and `linux.rs`**

- **Windows (`rust/src/driver/windows.rs`):**
  - `window_state`: query `GetForegroundWindow` -> `GetWindowThreadProcessId` -> `resolve_proc_name` + `get_app_windows` count & foreground rect (`<2ms`, no UIA tree walk).
  - `SetValue`: `verify_point_safe` -> `element_from_point` -> verify `UIA_VALUE_PATTERN_ID` or editable UIA control type (`UIA_EDIT_CONTROL_TYPE_ID = 50004` / `UIA_DOCUMENT_CONTROL_TYPE_ID = 50030`) before writing via `IUIAutomationValuePattern::SetValue` (supporting `""`), or verified fallback `Ctrl+A` + `Delete` (if non-empty, `send_unicode_text_batched`); fail closed (`target_identity_unverified`) if element is not editable.
  - `SelectText`: `verify_point_safe` -> verify editable element -> `IUIAutomationTextPattern` range selection or verified `Ctrl+A` when `start`/`end` omitted.
  - `send_unicode_text`: batch up to 32 UTF-16 `KEYEVENTF_UNICODE` down+up `INPUT` structs per `SendInput` syscall instead of calling `SendInput` one character at a time.
  - `capture_and_ocr` + `render_som_image`: resize if `w > opts.max_width` and encode via `encode_rgba_image_b64` / `render_som_image_to_b64`.
- **Linux (`rust/src/driver/linux.rs`):**
  - `window_state`: read X11 active window (`_NET_ACTIVE_WINDOW`, `_NET_WM_PID`, `XGetWindowAttributes`) directly via `DisplayGuard` (`dlopen` `libX11`, zero Python subprocess!) or minimal env fallback on Wayland.
  - Extend embedded AT-SPI2 helper (`set_value` & `select_text` subcommands) to enforce `ROLE_PASSWORD_TEXT` guard and use `Atspi.EditableText.set_text_contents` / `select_text` (`Atspi.Text.set_selection`), returning `{ok:false, code:"target_identity_unverified"}` when target is not editable. When `set_value` confirms editable via AT-SPI or web fallback, support `""` clearing via `Ctrl+A` + `BackSpace`.
  - Fast typing: remove `--delay 2` on `xdotool` (`--delay 0`), batch `xtest` flush.
  - `capture_and_ocr` + `render_som_image`: downscale & encode via `encode_rgba_image_b64` / `render_som_image_to_b64`.
  - Also run cross-check `cargo check` for Linux/Windows if targets installed locally (`rustup target list --installed` check; run standard `cargo test` unconditionally).

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path rust/Cargo.toml`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add rust/src/driver/windows.rs rust/src/driver/linux.rs
git commit -m "feat(windows,linux): implement SetValue, SelectText, WindowState, batched typing, and JPEG encoding"
```

---

### Task 5: Gateway Speed & Ergonomics Engine + MCP Schema (`gateway.rs`, `mcp.rs`)

**Files:**
- Modify: `rust/src/gateway.rs`
- Modify: `rust/src/mcp.rs:7-265`

**Interfaces:**
- Consumes: `crate::config::AppConfig`, `ActionRequest::{SetValue, SelectText}`, `OsDriver::window_state`, `ImageEncodeOptions`
- Produces:
  - `pub fn Gateway::with_config(driver: Box<dyn OsDriver>, settings: PolicySettings, state_dir: PathBuf, config: AppConfig) -> Self` (preserving `Gateway::new(...)` delegating to `AppConfig::default()`)
  - `ToolOutput { pub payload: JsonValue, pub image_b64: Option<String>, pub image_mime_type: &'static str, pub image_caption: Option<String> }`
  - `mcp.rs`: updated `tool_defs()` documenting `set_value`, `select_text`, `target`, `keep_snapshot`, `format`, `quality`, `max_width`, `run_actions` (`<=24` steps, inline `wait_for`/`set_value`/`select_text`/`target`), and emitting `image_mime_type` on MCP `ImageContent`.

- [ ] **Step 1: Write failing unit tests in `gateway.rs` and `mcp.rs`**

Add unit tests covering all new engine behaviors and every Review Focus gateway edge case:
1. **`set_value` & `select_text` flow + safety:**
   - `set_value` succeeds on `TextArea`/`TextField` `ref` including `text: ""` clearing, logs `SetValue` in `MockDriver`, rejects `""` on `type`, and fails closed (`target_identity_unverified`) on raw `(x, y)` or OCR-only mark `[m3]`.
2. **Semantic `target: { query, role }`:**
   - Reuses cached unfiltered snapshot when app matches (`inspect_count == 1` across inspect + target action), performs 1 automatic re-inspect if cached lookup misses, prefers exact case-insensitive label match (`"Save"` beats `"Save As..."`), and fails closed with `ambiguous_target` when multiple partial matches or multiple exact matches exist.
3. **Expanded `run_actions` (`<=24` steps + inline `wait_for` / `set_value` / `select_text` / `target`):**
   - Executes >12 steps (e.g. 14 steps) including `set_value`, `select_text`, `wait_for`, and `click` with `target` in one call.
4. **Adaptive `verify_change` early exit:**
   - Exits within first poll tick (~`30ms`) rather than sleeping full `settle_ms` when mock/test driver reports change on tick 1.
5. **Lightweight `window_state` form-ref preservation (`reuse_form_refs` / `keep_snapshot`):**
   - Preserves both `elements_by_ref` and `marks_by_id` across consecutive `type` / `set_value` / `select_text` calls when `window_state` stays unchanged (including under background target or `confirmation: required`, consuming single-use confirmation token cleanly).
   - Invalidates both (`stale_ref`) when mock `window_state` simulates `frontmost_bundle_id` changing mid-form!
6. **Default `image/jpeg` + opt-in `format: "png"` in `mcp::tests`:**
   - Default `visual_inspect` / `screenshot` emit `mimeType: "image/jpeg"` and `image_format: "jpeg"` in `structuredContent`, while `arguments.format = "png"` emits `mimeType: "image/png"`.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path rust/Cargo.toml gateway::tests mcp::tests`
Expected: FAIL

- [ ] **Step 3: Implement Gateway & MCP updates in `rust/src/gateway.rs` and `rust/src/mcp.rs`**

- Generalize `ToolOutput` to `image_b64` + `image_mime_type` (`handle_rpc_line` emits `out.image_mime_type`).
- Update `resolve_image_options(args, &self.config)` validating `format` (`"jpeg"|"png"`), `quality` (`1..=100`), `max_width` (`640..=2560`), recording `image_format`, `image_width`, `image_height`, `image_bytes` on `visual_inspect` & `screenshot` responses, and transcoding `MockDriver` captures cleanly.
- Implement `set_value`, `select_text`, semantic `resolve_semantic_target`, adaptive `execute_verified_request` (`poll_interval_ms` polling up to `settle_ms`), `run_actions` (`1..=24` steps, inline `wait_for`, `set_value`, `select_text`, `target`), and `snapshot` preservation gated by `window_state` match (`frontmost_bundle_id`, `pid`, `windows_total`, `active_window_frame`).
- Record `snapshot_frontmost: Option<String>` and `snapshot_active_frame: Option<Rect>` inside `record_snapshot_meta` so `window_state` comparisons require zero assumptions!

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path rust/Cargo.toml`
Expected: PASS (all unit tests including existing policy/som/memory tests green)

- [ ] **Step 5: Commit**

```bash
git add rust/src/gateway.rs rust/src/mcp.rs
git commit -m "feat(gateway,mcp): add set_value, select_text, semantic target, adaptive verify, JPEG default, and form ref reuse"
```

---

### Task 6: Universal Agent `install`, `config`, `call`, `doctor --check` CLI & E2E Tests (`installer.rs`, `main.rs`, `tests/test_rust_mcp.py`)

**Files:**
- Create: `rust/src/installer.rs`
- Modify: `rust/src/lib.rs`, `rust/src/main.rs:1-149`
- Modify: `tests/test_rust_mcp.py:1-240`

**Interfaces:**
- Consumes: `crate::config::{AppConfig, resolve_config_path}`, `crate::gateway::Gateway`, `crate::json::{parse_json, JsonValue}`
- Produces in `rust/src/installer.rs`:
  - `pub const SUPPORTED_AGENTS: &[&str]` (`antigravity`, `gemini`, `claude`, `claude-desktop`, `codex`, `cursor`, `windsurf`, `vscode`, `opencode`)
  - `pub struct InstallOptions { pub write: bool, pub dry_run: bool, pub scope: InstallScope, pub use_npx: bool, pub policy_path: Option<PathBuf>, pub with_skill: bool }`
  - `pub fn install_agent(agent: &str, exe_path: &Path, opts: &InstallOptions) -> Result<JsonValue, String>`
  - `pub fn install_all(exe_path: &Path, opts: &InstallOptions) -> Result<JsonValue, String>`
  - Embedded skill constant via `include_str!("../../.agents/skills/apexuse/SKILL.md")`

- [ ] **Step 1: Write failing Rust unit tests (`installer::tests`) and Python E2E tests (`tests/test_rust_mcp.py`)**

In `rust/src/installer.rs`, pin unit tests including Review Focus cases:
- Merging `apexuse` into an existing `{"mcpServers": {"other": {"command": "foo"}}}` JSON preserves `"other"` untouched and creates `.bak`.
- Rejecting commented/corrupt JSON (`{\n // comment\n "mcp": {} }`) without touching or overwriting the original file!
- Upserting Codex TOML `[mcp_servers.apexuse]` & `[mcp_servers.apexuse.env]` when sibling `[mcp_servers.context7]` and top-level `model = "gpt-5"` exist — replacing old `apexuse` blocks idempotently without losing sibling servers or top-level settings.
- Formatting OpenCode (`"mcp": { "apexuse": { "type": "local", "command": [...], "environment": {...}, "enabled": true } }`), VS Code (`"servers"`), and `--npx` (`"command": "npx"`, `"args": ["-y", "apexuse-mcp"]`).
- `--with-skill` writes `SKILL.md` to temp target skill dir cleanly.

In `tests/test_rust_mcp.py`:
- Update existing `visual_inspect` / `screenshot` assertions for default `image/jpeg` + add test for explicit `format="png"`.
- Add E2E assertions for `set_value` (`text=""` clear + value set), `select_text`, semantic `target`, `keep_snapshot` form batching, and CLI subcommands (`apexuse-mcp --version`, `config set/get`, `call --mock --calls '...'`, `install cursor --dry-run --npx`, `doctor --check`).

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path rust/Cargo.toml installer::tests`
Expected: FAIL (`installer` module missing)

- [ ] **Step 3: Implement `rust/src/installer.rs`, update `rust/src/main.rs` & `rust/src/lib.rs`, and update `tests/test_rust_mcp.py`**

Wire CLI parsing cleanly in `rust/src/main.rs`:
- `-V` / `--version` -> print `apexuse-mcp 0.3.0`.
- `config show | get <key> | set <key> <val> | reset` (honoring `APEXUSE_STATE_DIR` / `APEXUSE_CONFIG`).
- `install <agent|all>` with `--write`, `--dry-run`, `--scope user|project`, `--npx`, `--policy <path>`, `--with-skill`.
- `doctor [--check]` (exit 1 when `accessibility == false || screen_recording == false` under `--check`).
- `call <tool> [--args '<json>'] [--save-image <path>]` and `call --calls '<json>' | --calls-file <path> [--sleep-ms <n>] [--mock]`, constructing `Gateway::with_config(...)` once so snapshot refs & marks persist across steps.

- [ ] **Step 4: Build release binary & run full Rust + Python E2E test suites**

Run: `cargo test --manifest-path rust/Cargo.toml && cargo build --release --manifest-path rust/Cargo.toml && .venv/bin/pytest -q`
Expected: PASS (all Rust unit tests + all Python MCP stdio & CLI E2E tests pass)

- [ ] **Step 5: Commit**

```bash
git add rust/src/installer.rs rust/src/lib.rs rust/src/main.rs tests/test_rust_mcp.py
git commit -m "feat(cli,installer): add auto-merging multi-agent installer, config/call CLI, and E2E tests"
```

---

### Task 7: Zero-Dep `npx` Launcher, `macos-x86_64` Release Pipeline, and Docs (`npm/apexuse-mcp/*`, `.github/workflows/release.yml`, docs)

**Files:**
- Create: `npm/apexuse-mcp/package.json`, `npm/apexuse-mcp/bin/apexuse-mcp.js`, `npm/apexuse-mcp/README.md`, `npm/apexuse-mcp/LICENSE`, `npm/apexuse-mcp/test/launcher.test.js`
- Modify: `.github/workflows/release.yml:10-36`
- Modify: `README.md`, `agent-configs/README.md`, `llms.txt`, `.agents/skills/apexuse/SKILL.md`

**Interfaces:**
- `npm/apexuse-mcp/bin/apexuse-mcp.js`: exports helper functions (`resolveTarget(platform, arch)`, `parseSha256(text)`, `stripWrapperFlags(args)`, `resolveBinary(env, pathLookup, downloader)`) for unit testing alongside CLI entrypoint guarded by `require.main === module` (so `stdio` exec happens cleanly and unit tests never hit network).

- [ ] **Step 1: Write failing Node built-in `node:test` suite (`npm/apexuse-mcp/test/launcher.test.js`)**

Cover Review Focus edge cases:
- Target resolution maps `darwin/arm64 -> macos-arm64.tar.gz`, `darwin/x64 -> macos-x86_64.tar.gz`, `linux/x64 -> linux-x86_64.tar.gz`, `win32/x64 -> windows-x86_64.zip`, rejects unsupported platform/arch clearly.
- `stripWrapperFlags(["--use-system", "doctor", "--check"])` extracts `useSystem: true` and returns forwarded `["doctor", "--check"]` (and keeps `--mock`, `install`, etc. untouched).
- `parseSha256` parses both Unix `"<hex>  file.tar.gz"` and PowerShell/uppercase checksum output, rejecting corrupted/empty digests.
- `resolveBinary` uses `APEXUSE_BIN`, cached version binary, mocked download with digest mismatch error cleanly failing before extraction.

- [ ] **Step 2: Run Node test to verify failure**

Run: `node --test npm/apexuse-mcp/test/launcher.test.js`
Expected: FAIL (`MODULE_NOT_FOUND`)

- [ ] **Step 3: Implement `npm/apexuse-mcp/` launcher, update `.github/workflows/release.yml`, and sync documentation**

- Write `npm/apexuse-mcp/package.json` (`"version": "0.3.0"`, `"dependencies": {}`, `"bin": {"apexuse-mcp": "bin/apexuse-mcp.js", "apexuse": "bin/apexuse-mcp.js"}`), copy `LICENSE`, write concise `npm/apexuse-mcp/README.md`, implement `bin/apexuse-mcp.js` (HTTP redirect following for GitHub Releases, SHA-256 verification before `tar -xzf` / PowerShell `Expand-Archive`, atomic binary move to `~/.apexuse/bin/v0.3.0/`, all diagnostics to `stderr`, signal forwarding on child spawn).
- Update `.github/workflows/release.yml`: update tag defaults `v0.2.0` -> `v0.3.0`, add `macos-x86_64` matrix row (`os: macos-latest`, `rust_target: x86_64-apple-darwin`, running `rustup target add x86_64-apple-darwin` + `cargo build --release --target x86_64-apple-darwin` -> binary path `rust/target/x86_64-apple-darwin/release/apexuse-mcp`, guarding `cargo test` with `if: matrix.rust_target == ''` so cross-compile rows skip running non-native binaries on ARM64).
- Update `README.md`, `agent-configs/README.md`, `llms.txt`, and `.agents/skills/apexuse/SKILL.md` covering `npx -y apexuse-mcp`, `install <agent> --write --with-skill` (all 9 clients + `all`), `call`, `config`, `set_value`/`select_text`, semantic `target`, and JPEG speed mode.

- [ ] **Step 4: Run all test suites & local launcher smoke test**

Run:
- `node --test npm/apexuse-mcp/test/launcher.test.js`
- `APEXUSE_BIN="$PWD/rust/target/release/apexuse-mcp" node npm/apexuse-mcp/bin/apexuse-mcp.js --use-system --version`
- `cargo test --manifest-path rust/Cargo.toml`
- `cargo build --release --manifest-path rust/Cargo.toml`
- `./rust/target/release/apexuse-mcp doctor --check` (or `doctor` if terminal permissions unset)
- `.venv/bin/pytest -q`

Expected: All tests pass (`0` failures), launcher prints `apexuse-mcp 0.3.0`.

- [ ] **Step 5: Commit**

```bash
git add npm/ .github/workflows/release.yml README.md agent-configs/README.md llms.txt .agents/skills/apexuse/SKILL.md
git commit -m "feat(npm,release,docs): add npx launcher, macos-x86_64 release build, and v0.3.0 docs"
```
