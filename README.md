# handsmcp

An experimental macOS computer-use MCP for Antigravity and other agents. The Cua preview combines Munim inspection and input checks with Cua button clicks. It is a prototype, not yet a general-purpose, reliable controller for every app.

The macOS prototype uses Munim Computer Use. The foundation comparison and live checks are in `docs/ADR-001-foundation.md` and `docs/BENCHMARK.md`. `runtime/munim-computer-use` is locally built and ignored by Git; the upstream Apache-2.0 license is retained in `licenses/`.

## Current safety behavior

Calculator button input and TextEdit typing require a single-use, expiring confirmation token bound to the current snapshot. The agent must ask the user and must never confirm on its own. A new inspection or a cooperating handsmcp process's input invalidates pending snapshot confirmations. Before input, the server re-reads the target and requires the rendered snapshot to match except for changes to the frontmost-window flag; this is not an atomic target-identity guarantee. An advisory lock coordinates handsmcp processes, but cannot detect external desktop actions. Always re-inspect after unexpected changes. If a known secure-field marker appears, the complete text response is suppressed. Unrecognized secure-field representations can still evade this check; it is not a complete secret-protection boundary.

Requests are recorded in `~/.handsmcp/logs/audit.jsonl` with action, decision and code; typed content and application data are not logged. Create `~/.handsmcp/STOP` to refuse calls. Each server process limits requests to 120 per minute; that limit is not shared between processes. `policy.yaml` also supports `read_only` and `dry_run` modes and confirmation expiry; changes take effect when the server restarts. This is an early prototype, not a complete security boundary.

See `docs/INSTALL.md` for the pinned developer setup. Run tests with `.venv/bin/python -m pytest -q`. For development, start the stdio server with `PYTHONPATH=src .venv/bin/python -m handsmcp.server`. Workspace-local Antigravity config and a skill live under `.agents/`. The launcher is also registered in the local Antigravity IDE/CLI global MCP config; an Antigravity agent successfully called the desktop application-list tool after allowing the request in the IDE. Run `.venv/bin/handsmcp install --agent antigravity` (or `gemini`, `claude`, `codex`) to print a configuration; it does not write global settings. Other examples are in `agent-configs/README.md`. Windows and Linux are untested.

Use `inspect_app` for read-only inspection; it accepts an app bundle ID and optional `query` and `window`. Antigravity successfully inspected a local Chrome fixture with it, though the IDE still requested permission. Through Antigravity, replacing text in an initially unfocused fixture field succeeded after the focus retry patch. Secure-field detection can suppress snapshots, and other websites and workflows are not guaranteed to work.

The local wildcard permits inspection of any syntactically valid app bundle ID except those matching the known protected-app name patterns. Those patterns and secure-field detection are incomplete; inspection may expose private application content to the connected agent. Input support is limited by the implemented actions and policy.

## Cua Driver readiness (experimental)

Run `.venv/bin/handsmcp doctor` to check whether Cua Driver is installed and has macOS Accessibility and Screen Recording permissions. It does not change permissions. `integration: experimental_hybrid` means the optional Cua backend also requires the Munim runtime for safety checks. A ready permission result is not proof of desktop control.

To print a configuration for the Cua preview after installing Cua Driver and setting up this project, run `.venv/bin/handsmcp install --agent antigravity --backend cua`. Supported agent values are `antigravity`, `gemini`, `claude`, and `codex`. This prints configuration without writing client settings. The generated paths use this project's location; configure and test each client on its own machine. The Cua preview uses Munim inspection and policy checks, then can route an unambiguously matched button click to Cua. Antigravity has exercised Calculator buttons and a local Chrome fixture. Visible cursor behavior, broad app compatibility, and other agent integrations remain unverified.
