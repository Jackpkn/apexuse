# ApexUse v0.3.0 Phase 2: `cua_repl`-Grade AX Speed, Diffing & Single-Call State Plan

> **For agentic workers:** Execute via `superpowers:subagent-driven-development` on branch `feat/v0.3.0-wiring-speed`. Keep 0 external Rust crates (`rust/Cargo.toml`) and 0 external npm runtime dependencies (`npm/apexuse-mcp/package.json`).

**Goal:** Eliminate redundant AX IPC round-trips, redundant `inspect_app` calls, pixel-heavy verification captures, and full-tree token bloat on browser/native workflows by adding batched macOS AX attribute fetching + Chrome `AXURL`/table semantics, incremental AX diffing (`~`, `+`, `Removed refs:`), inline `return_state` on `computer`/`desktop`/`run_actions`, and `verify_mode: "ax" | "visual" | "auto"`.

---

### Task 8: Config Keys, Batched macOS AX IPC, `UiElement.url`, & AX Diff Engine (`config.rs`, `macos_bridge.m`, `driver/`)

**Files:**
- Modify: `rust/src/config.rs` (`SnapshotConfig`: `ax_diff: bool` default `true`, `include_bounds: bool` default `false`)
- Modify: `rust/src/driver/mod.rs` (`UiElement.url`, `render_outline_with_options`, `AxDiffMode`, `AxDiffResult`, `reconcile_and_diff_snapshot`)
- Modify: `rust/native/macos_bridge.m` (`walk_ax_node` using `AXUIElementCopyMultipleAttributeValues`, `kAXURLAttribute`, targeted `ax_actions`, Chromium table/WebArea structure)
- Modify: `rust/src/driver/macos.rs`, `rust/src/driver/common.rs`, `rust/src/som.rs`, `rust/src/gateway.rs` (populate `url` on `UiElement` literals so `cargo test` compiles cleanly)

**Interfaces Exported for Task 9:**
- `SnapshotConfig { pub reuse_form_refs: bool, pub ax_diff: bool, pub include_bounds: bool }` (`config.rs`: `snapshot.ax_diff`, `snapshot.include_bounds`)
- `UiElement { ..., pub url: Option<String> }`
- `AppSnapshot::render_outline_with_options(&self, query: Option<&str>, include_bounds: bool) -> String`
- `pub enum AxDiffMode { Full, Diff, Unchanged }`
- `pub struct AxDiffResult { pub content: String, pub mode: AxDiffMode }`
- `pub fn reconcile_and_diff_snapshot(prev: Option<&AppSnapshot>, next: &mut AppSnapshot, next_ref_id: &mut usize, include_bounds: bool, force_full: bool) -> AxDiffResult`

---

### Task 9: Gateway AX Diff Engine, Inline `return_state`, AX-Only `verify_mode`, & MCP Schema (`gateway.rs`, `mcp.rs`)

**Files:**
- Modify: `rust/src/gateway.rs`
- Modify: `rust/src/mcp.rs`

**Core Behavior:**
1. **`inspect_app` (`handle_inspect_app` & `run_actions` `inspect`):**
   - Optional `diff: bool` (defaults to `self.config.snapshot.ax_diff` when `query.is_none()` and `prev_ax_snapshot` matches `(bundle_id, window)`) and optional `include_bounds: bool` (defaults to `self.config.snapshot.include_bounds`).
   - ALWAYS validates `check_frontmost_permitted`, `snapshot_redaction_reason`, AND `contains_secure_marker(&snap.render_outline(None))` BEFORE computing any diff so security redaction never misses unchanged elements.
   - Emits `"state_mode": "full" | "diff" | "unchanged"` on `inspect_app` output alongside `"content"`, and keeps `elements_by_ref` synchronized with the reconciled refs so both preserved refs (`e1`, `e2`) and new diff refs (`+ [eN]`) work seamlessly.
2. **Inline `return_state: "none" | "diff" | "full"` on `computer`, `desktop`, and `run_actions`:**
   - Defaults cleanly to `"none"` (preserving existing `invalidate_snapshot()` behavior and mock call counts unless `"diff"` or `"full"` is passed, or `inspect_after: true` on `run_actions`).
   - When `"diff"` or `"full"` is requested after executing an action (`click`, `double_click`, `right_click`, `hover`, `drag`, `type`, `set_value`, `select_text`, `press_key`, `scroll`, `activate`): captures fresh `inspect_app(&app, window, 150, 12)`, runs full frontmost + redaction + `contains_secure_marker` validation, updates `prev_ax_snapshot` + active `elements_by_ref` (`snapshot_active: true`), and includes `"content"` + `"state_mode"` directly in the action response — eliminating extra `inspect_app` MCP round-trips!
3. **AX-Only Fast `verify_mode: "auto" | "ax" | "visual"`:**
   - Added to `ActionVerifyOptions` and `parse_verify_options`. Under `"ax"`, skips `driver.capture_and_ocr` pixel digest captures completely; under `"visual"`, skips AX check; `"auto"` preserves `ax || visual`.
4. **`mcp.rs` `tool_defs()`:** Documents `diff`, `include_bounds`, `return_state`, and `verify_mode` cleanly, recommending AX-first diffing + `return_state: "diff"`.

---

### Task 10: E2E Tests, Skill/Docs Update, & Full Verification (`tests/test_rust_mcp.py`, docs)

**Files:**
- Modify: `tests/test_rust_mcp.py`
- Modify: `.agents/skills/apexuse/SKILL.md`, `README.md`, `agent-configs/README.md`, `llms.txt`, `npm/apexuse-mcp/README.md`

**Core Work:**
- Add Python E2E tests verifying `inspect_app` AX diff mode + `diff: false`, compact outlines (`@(...)` omitted vs `include_bounds: true`), `computer` with `return_state: "diff"` returning inline diff + keeping refs clickable without a separate `inspect_app`, `verify_mode: "ax"`, and `config set snapshot.ax_diff` / `snapshot.include_bounds`.
- Update `SKILL.md` (embedded into `installer.rs`) and docs to teach agents the fast `cua_repl`-style flow (`inspect_app` AX diff first -> `set_value`/`select_text`/semantic `target` -> `return_state: "diff"` -> `visual_inspect` JPEG fallback only when AX lacks labels).
- Rebuild `--release`, run `cargo check --all-targets` + cross-target `x86_64-pc-windows-gnu` & `x86_64-unknown-linux-gnu` (`0` warnings), `cargo test`, `node --test`, `.venv/bin/pytest -q`, and launcher smoke tests.
