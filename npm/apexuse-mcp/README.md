# apexuse-mcp

Zero-dependency `npx` launcher for **[ApexUse](https://github.com/Jackpkn/apexuse)** (`apexuse-mcp`) — a local-first cross-platform Computer Use MCP server in pure Rust with batched native Accessibility + incremental AX diffing (`state_mode`), single-round-trip `return_state: "diff"`, AX-only `verify_mode: "ax"`, Set-of-Mark (`[m1]..[mN]`) visual grounding, local OCR, batched `run_actions`, and workflow memory.

## Quick Start

Installing via `npm` (`npm install -g apexuse-mcp` or `npm install apexuse-mcp`) prints a `postinstall` setup banner with platform-specific next steps.

Start the MCP stdio server directly (auto-downloads and SHA-256-verifies the v0.3.0 binary into `~/.apexuse/bin/v0.3.0/` on first run):

```sh
npx -y apexuse-mcp
```

Wire detected AI coding agents (`antigravity`, `gemini`, `claude`, `claude-desktop`, `codex`, `cursor`, `windsurf`, `vscode`, `opencode`, or `all`), install `SKILL.md`, and verify the live MCP handshake with `--probe`:

```sh
npx -y apexuse-mcp install all --write --npx --with-skill --probe
```

Install as a **Codex Desktop Plugin** (materializes `~/.codex/plugins/` and configures `[marketplaces.apexuse-local]` + `[plugins."apexuse@apexuse-local"]` without duplicate `[mcp_servers.apexuse]`):

```sh
npx -y apexuse-mcp install codex-plugin --write --npx --probe
```

Verify OS permissions and live MCP readiness:

```sh
# macOS guided permission onboarding UI (host-aware Accessibility + Screen Recording)
npx -y apexuse-mcp doctor --onboard

# Headless OS readiness check (exits non-zero when permissions/deps are missing)
npx -y apexuse-mcp doctor --check

# Standalone live MCP initialize + tools/list probe
npx -y apexuse-mcp probe --npx
```

Install the full agent skill pack (`SKILL.md`, `agents/openai.yaml`, `references/`):

```sh
npx skills add Jackpkn/apexuse -g -a codex --skill apexuse -y
```

## Launcher Options & Environment

- `--use-system`: Look up `apexuse-mcp` on `PATH` if no cached binary exists before downloading (stripped before forwarding args to `apexuse-mcp`).
- `APEXUSE_BIN`: Explicit path to a local `apexuse-mcp` binary (highest priority).
- `APEXUSE_STATE_DIR`: Custom state/cache directory (defaults to `~/.apexuse`).
- `APEXUSE_POLICY`: Path to `policy.yaml` / `policy.autonomous.yaml`.

Supported prebuilt release targets (`v0.3.0`):
- `macos-arm64` (`darwin` / `arm64`)
- `macos-x86_64` (`darwin` / `x64`)
- `linux-x86_64` (`linux` / `x64`)
- `windows-x86_64` (`win32` / `x64`)

For full CLI (`config` including `snapshot.ax_diff` / `snapshot.include_bounds`, `call`), MCP tool schemas (`inspect_app` diffing, `return_state: "diff"`, `verify_mode: "ax"`, `set_value`, `select_text`, semantic `target`, `keep_snapshot`, `run_actions`, JPEG default), and safety controls, see the [main repository README](https://github.com/Jackpkn/apexuse#readme).

## License

MIT — see [LICENSE](./LICENSE).
