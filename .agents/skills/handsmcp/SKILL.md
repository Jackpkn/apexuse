---
name: handsmcp
description: Use for cross-app desktop and browser computer-use via Accessibility + Set-of-Mark visual indexing + workflow memory when shell/files are insufficient.
---

# handsmcp (Hybrid Accessibility + Set-of-Mark Computer Use)

Use the `handsmcp` MCP tools for desktop, Electron, and browser interaction across all permitted apps using an independent on-screen `AgentCursor` and per-PID event routing:

1. **Discover & Inspect First**:
   - Call `desktop` (`action: "list_applications"`) to see running apps, or `launch_application` / `activate_application` with an exact bundle ID.
   - Prefer `visual_inspect` (`app: "<bundle_id>"` or `"desktop"`) to capture a Set-of-Mark annotated image (`[m1]..[mN]`) fusing Accessibility boxes + Apple Vision OCR, or `inspect_app` (`app`, optional `window`, `query`) for semantic `[e1]..` refs and learned `workflow_hints`.
   - Call `workflow_memory` (`action: "lookup"`, `app`) before unfamiliar flows, and `record` reusable non-secret workflows afterward.

2. **Act Across Apps**:
   - `computer` actions: `click`, `double_click`, `right_click`, `hover` (accepts `ref` or `mark`), `drag` (`from_ref`/`from_mark` -> `to_ref`/`to_mark`), `type` (`ref` or `mark`, `text`, optional `mode="insert"|"replace"`), `press_key` (`key`, optional `modifiers`), `scroll` (`direction`, `amount`), `inspect`.
   - `desktop` actions: `visual_click`, `double_click`, `right_click`, `hover`, `drag` (supports `mark`, `ref`, or `x`/`y` coordinates after an unfiltered snapshot).
   - Under `policy.autonomous.yaml` (`confirmation: direct`), actions execute directly without per-click confirmation loops. **Only** if a stricter policy returns `confirmation_required` should you show the summary to the user and wait before calling `confirm`.
   - Always re-inspect (`visual_inspect` or `inspect_app`) after state-changing actions to verify the result.

3. **Safety Boundaries (Always Enforced)**:
   - Protected password-manager, wallet, keychain, and banking apps are blocked.
   - `AXSecureTextField`, unverified text fields, and masked bullet fields are redacted/blocked; never enter or store passwords, tokens, OTPs, or secrets.
   - Clipboard paste shortcuts (`Cmd+V` / `Ctrl+V`) are blocked; use `computer(action="type")`.
   - `~/.handsmcp/STOP` immediately halts mutations.
