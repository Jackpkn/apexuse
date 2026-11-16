# Developer installation (macOS, experimental)

This installs an experimental prototype. Some Chrome and desktop actions work, but general desktop control is not yet reliable or complete.

Requirements: macOS 14+, Git, Swift 5.9+ and Python 3.11+. Run the installer from a checked-out copy of this project:

```sh
./scripts/install.sh
.venv/bin/handsmcp install --agent antigravity --backend cua
```

The installer builds the pinned upstream Munim source with the included patches and installs the Python package in `.venv`. It preserves an existing runtime and virtual environment. It requires network access for the source and Python dependencies. Install Cua Driver separately using the official instructions at https://cua.ai/docs/cua-driver/quickstart for the Cua backend, then run `.venv/bin/handsmcp doctor` to check readiness. The second command prints configuration; it does not modify the agent's settings. Follow `agent-configs/README.md` to merge that configuration into your agent. This project is not yet published as an installable release.

The project's workspace-local `.agents/mcp_config.json` contains absolute paths for this Mac and is deliberately ignored by Git; see `agent-configs/README.md` for other machines and agents. The server runs locally over stdio; launching it requires `PYTHONPATH` to include `src`. The included MCP smoke test verifies a local client and the server, not the Antigravity application. Antigravity integration has been exercised for selected actions; other clients need independent verification.

For full desktop use macOS may require Accessibility and Screen Recording for the relevant agent or launcher. These permissions are independent of MCP connection. App-window screenshot capture has been verified from Antigravity for a test fixture and TextEdit; capture permissions are launcher-dependent.

The patch deliberately disables upstream OCR and remote/OCR-target typing until those paths can be made safe. It fails closed on text fields whose security subrole cannot be verified, which can block ordinary Chrome fields.

Chrome field classification has been exercised on a synthetic ordinary input and a separate password input. Insertion and an Apply button were tested in a local Chrome practice page through both a local MCP client and Antigravity; an earlier Antigravity insertion attempt did not change the field. After the focus-retry patch, replacement also succeeded from an initially unfocused test field through Antigravity. Other sites and agent clients remain unverified.
