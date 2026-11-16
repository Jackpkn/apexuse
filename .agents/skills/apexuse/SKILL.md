---
name: apexuse
description: Use for cross-app desktop and browser computer-use via Accessibility + Set-of-Mark visual indexing + workflow memory when shell/files are insufficient.
---

# ApexUse (Hybrid Accessibility + Set-of-Mark Computer Use)

Use the `apexuse` MCP tools for desktop, Electron, and browser interaction across all permitted apps using an independent on-screen `AgentCursor` and per-PID event routing:

1. **Discover & Inspect Fast**:
   - Call `desktop` (`action: "list_applications"`) to see running apps, `launch_application` / `activate_application`, or `open_url` (`url: "https://..."`, optional `app`, defaults to Chrome) to open web pages directly without clicking the address bar.
   - Prefer `inspect_app` (`app`, optional `query`, no screenshot) when Accessibility semantics suffice; use `visual_inspect` (`app: "<bundle_id>"` or `"desktop"`, optional `ocr: "auto"|"fast"|"accurate"|"off"`, optional `max_marks`) when you need visual `[m1]..[mN]` Set-of-Mark grounding (`ocr: "off"` skips OCR for instant AX-only visual marks; `"fast"` runs rapid Vision OCR).
   - Call `workflow_memory` (`action: "lookup"`, `app`) before unfamiliar flows, `record` reusable non-secret workflows afterward, and `replay` (`id` or `app`+`goal`, optional `step_args`) to execute learned macros in one turn.

2. **Act & Chain Across Apps**:
   - `computer` actions: `click`, `double_click`, `right_click`, `hover` (accepts `ref`, `mark`, or `x`/`y` after an unfiltered snapshot), `drag` (`from_ref`/`from_mark`/`x1`,`y1` -> `to_ref`/`to_mark`/`x2`,`y2`), `type` (`ref` or `mark`, `text`, optional `mode="insert"|"replace"`), `press_key` (`key`, optional `modifiers`), `scroll` (`direction`, `amount`, optional target `ref`/`mark`/`x`,`y` for inner scrollable panes), `inspect`, `run_actions`, `wait_for`.
   - Any action or `run_actions` step accepts optional `verify_change: true`, `max_retries: 0..=3`, `settle_ms`, `retry_delay_ms`, and `require_change: true` to compare pre/post Accessibility & pixel digests and auto-retry if the UI did not change.
   - Use `computer` (`action: "run_actions"`, `arguments: { steps: [...], inspect_after?: true }`) to chain up to 12 deterministic steps (`click`, `type`, `press_key`, `scroll`, `sleep_ms`, `activate_application`, etc.) locally in a single turn with zero LLM round-trips between steps.
   - Use `computer` (`action: "wait_for"`, `arguments: { app, query, window?, timeout_ms?, poll_ms?, absent?, ocr? }`) to wait locally for uploads, page loads, app launches, or spinners instead of sleeping in model turns.
   - `desktop` actions: `open_url`, `visual_click`, `double_click`, `right_click`, `hover`, `drag` (supports `mark`, `ref`, or `x`/`y` coordinates after an unfiltered snapshot).
   - Under `policy.autonomous.yaml` (`confirmation: direct`), actions execute directly without per-click confirmation loops (with single-point safety verification on macOS). **Only** if a stricter policy returns `confirmation_required` should you show the summary to the user and wait before calling `confirm`.
   - Re-inspect (`inspect_app` or `visual_inspect`) after major navigation, or pass `inspect_after: true` on `run_actions`.

3. **Safety Boundaries (Always Enforced)**:
   - Protected password-manager, wallet, keychain, and banking apps are blocked.
   - `AXSecureTextField`, unverified text fields, and masked bullet fields are redacted/blocked; never enter or store passwords, tokens, OTPs, or secrets.
   - Clipboard paste shortcuts (`Cmd+V` / `Ctrl+V`) are blocked; use `computer(action="type")`.
   - `~/.apexuse/STOP` immediately halts mutations.
