# ApexUse Tool & CLI Usage Guide

## 1. AX-First Inspection & Diffing (`inspect_app`)

1. Discover running apps or open URLs directly:
   - `desktop` with `action: "list_applications"`, `launch_application`, `activate_application`, or `open_url` (`url`, optional `app`, supports `return_state: "diff"`).
2. Prefer `inspect_app` (`app`, optional `window`, `query`, `diff` default `true`, `include_bounds` default `false`) before taking screenshots:
   - Emits compact element outlines with `e1..eN` refs, `URL=` on windows/`WebArea`/links, and Chrome/Electron `Table`/`Row`/`Cell`/`ColumnHeader` and `Tab` structure.
   - Auto-diffs repeat views of the same app/window (`state_mode`: `"full" | "diff" | "unchanged" | "filtered"`), keeping `e1..eN` refs live.
   - Pass `diff: false` to force a fresh full tree, and `include_bounds: true` only when raw `@(x,y,wxh)` geometry is needed.

## 2. Single-Round-Trip Actions (`return_state: "diff"`)

Pass `return_state: "diff"` (or `"full"`, default `"none"`) on `computer`, `desktop`, or `run_actions`:
- Executes the action and returns post-action `content`, `state_mode`, `snapshot_active: true`, and `snapshot_preserved: true` with refreshed refs in **one** MCP round-trip.
- Eliminates separate follow-up `inspect_app` calls.

## 3. Native Form Edits (`set_value`, `select_text`, `keep_snapshot`)

- `set_value`: writes or clears (`text: ""`) text directly on verified editable controls via native macOS AX / Windows UIA / Linux AT-SPI. Fails closed (`target_identity_unverified`) on raw `(x, y)` coordinates or OCR-only marks.
- `select_text`: selects the full field value or optional `[start, end)` character offsets.
- `keep_snapshot`: non-navigating edits (`type`, `set_value`, `select_text`) automatically preserve `e1..eN` and `[m1]..[mN]` refs when window state is unchanged (`snapshot.reuse_form_refs = true`; pass `keep_snapshot: false` to invalidate).

## 4. Semantic Targeting (`target`)

Pass `target: { query, role?, fresh? }` on `computer` (`click`, `double_click`, `right_click`, `hover`, `type`, `set_value`, `select_text`), `desktop`, or `run_actions` steps:
- Resolves against the active unfiltered snapshot first, performs one refresh if missing or `fresh: true`, and fails closed on `target_not_found` or `ambiguous_target`.

## 5. Multi-Step Batching (`run_actions`) & `wait_for`

- `computer` (`action: "run_actions"`) chains up to **24** steps (`click`, `set_value`, `type`, `select_text`, `press_key`, `scroll`, `wait_for`, `inspect`, `sleep_ms`, `activate_application`, etc.) in one turn.
- Supports top-level `return_state: "diff" | "full"` (or `inspect_after: true`) and per-step `verify_mode`, `verify_change`, `diff`, and `include_bounds`.
- `wait_for` (`app`, `query`, `role?`, `window?`, `timeout_ms?`, `poll_ms?`, `absent?`, `ocr?`) polls locally up to 60s without burning LLM turns.

## 6. AX-Only Verification (`verify_mode: "ax"`)

With `verify_change: true`, pass `verify_mode: "ax"` (vs `"auto"` or `"visual"`) to poll and confirm UI state transitions strictly through Accessibility with zero screenshots (`max_retries: 0..=3`, `settle_ms`, `poll_interval_ms`, `retry_delay_ms`, `require_change`).

## 7. Set-of-Mark Visual Grounding (`visual_inspect`)

Use `visual_inspect` (`app` or `"desktop"`, optional `ocr: "auto"|"fast"|"accurate"|"off"`, `max_marks` `1..=150`, `format: "jpeg"|"png"`, `quality`, `max_width`) when visual layout or OCR text is required:
- Overlays `[m1]..[mN]` badges; defaults to compact JPEG (`quality=80`, `max_width=1280`).
- Pass `ocr: "off"` for instant AX-only visual marks.

## 8. Workflow Memory (`workflow_memory`)

- `lookup` (`app`) before unfamiliar flows, `record` non-secret structural steps afterward, and `replay` (`id` or `app` + `goal`, optional `step_args`) to execute learned macros in one call (`list` and `clear` also supported).

## 9. Direct CLI (`config` & `call`)

```sh
# Persistent runtime configuration (~/.apexuse/config.json or $APEXUSE_CONFIG)
npx -y apexuse-mcp config show
npx -y apexuse-mcp config set image.format png
npx -y apexuse-mcp config set snapshot.ax_diff true
npx -y apexuse-mcp config set snapshot.include_bounds false
npx -y apexuse-mcp config reset

# One-off or stateful chained tool calls from shell
npx -y apexuse-mcp call inspect_app --args '{"app":"Calculator"}'
npx -y apexuse-mcp call visual_inspect --args '{"app":"Calculator"}' --save-image out.jpg
npx -y apexuse-mcp call --calls '[{"tool":"inspect_app","args":{"app":"Calculator"}},{"tool":"computer","args":{"action":"click","arguments":{"target":{"query":"7"},"return_state":"diff","verify_mode":"ax"}}}]'
```
