# apexuse-mcp

Zero-dependency `npx` launcher for **[ApexUse](https://github.com/Jackpkn/apexuse)** (`apexuse-mcp`) — a local-first cross-platform Computer Use MCP server in pure Rust with native Accessibility, Set-of-Mark (`[m1]..[mN]`) visual grounding, local OCR, batched `run_actions`, and workflow memory.

## Quick Start

Start the MCP stdio server directly (auto-downloads and SHA-256-verifies the v0.3.0 binary into `~/.apexuse/bin/v0.3.0/` on first run):

```sh
npx -y apexuse-mcp
```

Or wire any AI coding agent automatically (`antigravity`, `gemini`, `claude`, `claude-desktop`, `codex`, `cursor`, `windsurf`, `vscode`, `opencode`, or `all`):

```sh
npx -y apexuse-mcp install all --write --npx --with-skill
```

Check OS permissions (`--check` exits non-zero when Accessibility or Screen Recording is unset):

```sh
npx -y apexuse-mcp doctor --check
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

For full CLI (`config`, `call`), MCP tool schemas (`set_value`, `select_text`, semantic `target`, `keep_snapshot`, `run_actions`, JPEG default), and safety controls, see the [main repository README](https://github.com/Jackpkn/apexuse#readme).

## License

MIT — see [LICENSE](./LICENSE).
