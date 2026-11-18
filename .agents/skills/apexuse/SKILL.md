---
name: apexuse
description: Use for cross-app desktop and browser computer-use via Accessibility + Set-of-Mark visual indexing + workflow memory when shell/files are insufficient.
---

# ApexUse (Hybrid Accessibility + Set-of-Mark Computer Use)

Use the `apexuse` MCP tools for desktop, Electron, and browser interaction across all permitted apps using an independent on-screen `AgentCursor` and per-PID event routing:

1. **Inspect & Discover Fast (AX-First & Diff-First)**:
   - Call `desktop` (`action: "list_applications"`) to see running apps, `launch_application` / `activate_application`, or `open_url` (`url: "https://..."`, optional `app`, defaults to Chrome; supports `return_state: "diff"`) to open web pages directly without clicking the address bar.
   - Prefer `inspect_app` (`app`, optional `window`, `query`, `diff` default `true`, `include_bounds` default `false`, zero screenshot) over screenshots whenever Accessibility semantics suffice:
     - Batches OS attribute IPC and exposes `URL=...` on windows/`WebArea`/links plus Chrome/Electron `Table`/`Row`/`Cell`/`ColumnHeader` and `Tab` structure.
     - Compact outlines omit `@(x,y,wxh)` by default (`include_bounds: false`); pass `include_bounds: true` only when raw geometry is needed.
     - Auto-diffs repeat views of the same app/window (`state_mode: "full" | "diff" | "unchanged" | "filtered"`) while keeping `e1..eN` refs live and reconciled; pass `diff: false` to force a fresh full tree.
   - Use `visual_inspect` (`app: "<bundle_id>"` or `"desktop"`, optional `ocr: "auto"|"fast"|"accurate"|"off"`, optional `max_marks`, optional `format: "jpeg"|"png"`, `quality`, `max_width`) only when visual `[m1]..[mN]` Set-of-Mark grounding or OCR is needed (`ocr: "off"` skips OCR for instant AX-only marks; defaults to fast compact JPEG `quality=80`, `max_width=1280`).
   - Call `workflow_memory` (`action: "lookup"`, `app`) before unfamiliar flows, `record` reusable non-secret workflows afterward, and `replay` (`id` or `app`+`goal`, optional `step_args`) to execute learned macros in one turn.

2. **Act & Chain in Minimal Round-Trips**:
   - **Inline `return_state` (prefer `"diff"` + `query`)**: Pass `return_state: "diff"` (plus optional `query` to filter returned lines while keeping refs live; avoid `"full"` unless `"diff"` is insufficient) on `computer` actions, `desktop` actions (`activate_application`, `launch_application`, `open_url`, `visual_click`, `double_click`, `right_click`, `hover`, `drag`), or `run_actions` to execute AND receive the post-action AX state (`content`, `state_mode`, `snapshot_active: true`, `snapshot_preserved: true`) with live refreshed refs in **one** MCP round-trip — eliminating separate post-action `inspect_app` calls. Avoid calling `inspect_app` on another app in parallel while mutating the target app, as it overwrites the active snapshot.
   - `computer` actions: `inspect` (supports `diff`, `include_bounds`, `window`, `query`), `click`, `double_click`, `right_click`, `hover` (accepts `ref`, `mark`, `target: {query, role?, fresh?}`, or `x`/`y` after an unfiltered snapshot; shifted labeled refs auto-reconcile once safely when unambiguous), `drag` (`from_ref`/`from_mark`/`x1`,`y1` -> `to_ref`/`to_mark`/`x2`,`y2`), `type` (`ref`, `mark`, or `target`), `set_value` (direct native AX/UIA/AT-SPI value assignment/clearing on verified editable controls; `text: ""` clears the field; fails closed on unverified coordinates or OCR-only marks), `select_text` (optional `start`/`end` offsets, or full text when omitted), `press_key`, `scroll` (`direction`, `amount`, optional target `ref`/`mark`/`x`,`y`), `run_actions`, `wait_for`, `confirm`.
   - **Semantic Targeting (`target`)**: Pass `target: { query, role?, fresh? }` on `computer` (`click`, `double_click`, `right_click`, `hover`, `type`, `set_value`, `select_text`), `desktop`, or `run_actions` steps to resolve elements deterministically without guessing coordinates (reuses active unfiltered snapshot first, performs one refresh on miss or `fresh: true`, and fails closed if `target_not_found` or `ambiguous_target`).
   - **Form Ref Reuse (`keep_snapshot`)**: Non-navigating edits (`type`, `set_value`, `select_text`) automatically preserve `e1..eN` and `[m1]..[mN]` across fields when the target window state stays unchanged (`config.snapshot.reuse_form_refs = true`, overridable per action with `keep_snapshot: false`).
   - **Multi-Step Batching (`run_actions`)**: Chain up to **24** deterministic steps (`click`, `set_value`, `type`, `select_text`, `press_key`, `scroll`, `wait_for`, `inspect`, `sleep_ms`, `activate_application`, etc.) in a single turn with zero LLM round-trips between steps; supports top-level `return_state: "diff"|"full"` (or `inspect_after: true`) and per-step `verify_mode`, `verify_change`, `diff`, and `include_bounds`.
   - **AX-Only Verification & Local Waits**: Any action or step supports `verify_change: true` with `verify_mode: "auto" | "ax" | "visual"` (`"ax"` verifies UI changes strictly via Accessibility with zero screenshots), `max_retries: 0..=3`, `settle_ms`, `poll_interval_ms`, `retry_delay_ms`, and `require_change: true`. Use `wait_for` (`app`, `query`, `role?`, `window?`, `timeout_ms?`, `poll_ms?`, `absent?`, `ocr?`) to wait locally up to 60s instead of sleeping in model turns.
   - `desktop` actions: `list_applications`, `launch_application`, `activate_application`, `open_url`, `visual_click`, `double_click`, `right_click`, `hover`, `drag` (supports `mark`, `ref`, `target`, or `x`/`y` coordinates after an unfiltered snapshot, plus `return_state`, `include_bounds`, and `verify_mode`).
   - Under `policy.autonomous.yaml` (`confirmation: direct`), actions execute directly without per-click confirmation loops. **Only** if a stricter policy returns `confirmation_required` should you show the summary to the user and wait before calling `confirm`.

3. **Safety Boundaries (Always Enforced)**:
   - Protected password-manager, wallet, keychain, and banking apps are blocked.
   - `AXSecureTextField`, unverified text fields, and masked bullet fields are redacted/blocked; never enter or store passwords, tokens, OTPs, or secrets.
   - Clipboard paste shortcuts (`Cmd+V` / `Ctrl+V`) are blocked; use `set_value` or `type`.
   - `~/.apexuse/STOP` immediately halts mutations.
