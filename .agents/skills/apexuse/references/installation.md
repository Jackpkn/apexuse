# ApexUse Installation & Agent Wiring

`apexuse-mcp` runs locally over MCP stdio with zero external Rust or npm runtime dependencies (Node.js `>=18` for `npx`).

## 1. Quick Start (`npx`)

```sh
# Wire all detected AI coding agents, install SKILL.md, and verify live MCP handshake
npx -y apexuse-mcp install all --write --npx --with-skill --probe
```

On first launch, `npx -y apexuse-mcp` downloads the `v0.3.0` release binary for `macos-arm64`, `macos-x86_64`, `linux-x86_64`, or `windows-x86_64`, verifies its `.sha256` checksum, and caches it under `~/.apexuse/bin/v0.3.0/`.

Alternative installs:
- `cargo install apexuse-mcp`
- `cargo build --release --manifest-path rust/Cargo.toml`
- Prebuilt archives from [GitHub Releases](https://github.com/Jackpkn/apexuse/releases)

Launcher overrides: `--use-system` (checks `PATH` before downloading), `APEXUSE_BIN`, `APEXUSE_STATE_DIR`, `APEXUSE_POLICY`.

## 2. 9-Agent Wiring (`install`)

Supported client targets: `antigravity`, `gemini`, `claude`, `claude-desktop`, `codex`, `cursor`, `windsurf`, `vscode`, `opencode`, or `all`.

```sh
# Preview merged config without writing
npx -y apexuse-mcp install cursor --dry-run --npx

# Write user-global config (creates .bak backup), install SKILL.md, and probe server
npx -y apexuse-mcp install cursor --write --npx --with-skill --probe

# Project-scoped wiring with custom policy
npx -y apexuse-mcp install claude --write --npx --scope project --policy ./policy.autonomous.yaml
```

Key flags:
- `--write`: safely validates JSON/TOML, writes `<target>.bak` for non-empty configs, and merges `"apexuse"`.
- `--dry-run`: prints the planned target path and merged snippet without modifying disk.
- `--scope <user|project>`: user-global (default) or workspace config (`claude-desktop` and `codex-plugin` are `user` scope only).
- `--npx`: sets `"command": "npx", "args": ["-y", "apexuse-mcp"]` instead of an absolute binary path.
- `--policy <path>`: adds `APEXUSE_POLICY=<path>` to the client's server `env` / `environment`.
- `--with-skill`: installs `SKILL.md` to `~/.agents/skills/apexuse/SKILL.md`, `~/.claude/skills/apexuse/SKILL.md`, and `~/.codex/skills/apexuse/SKILL.md` (or `.agents/` / `.claude/` in project scope).
- `--probe [--timeout-ms <n>] [--mock]`: spawns the configured command after install and verifies JSON-RPC `initialize` + `tools/list`.

## 3. Codex Desktop Plugin (`install codex-plugin`)

```sh
npx -y apexuse-mcp install codex-plugin --write --npx --probe
```

What `install codex-plugin --write` does:
1. Materializes `~/.codex/plugins/marketplaces/apexuse-local/` and `~/.codex/plugins/cache/apexuse-local/apexuse/0.3.0/` (manifest, icons, and `.mcp.json`).
2. Upserts `[marketplaces.apexuse-local]` and `[plugins."apexuse@apexuse-local"]` (`enabled = true`) in `~/.codex/config.toml`.
3. Removes `[mcp_servers.apexuse]` / `[mcp_servers.apexuse.env]` to avoid duplicate Codex server registrations.

Notes:
- Re-running `install all --write` detects active `[plugins."apexuse@apexuse-local"]` (`enabled = true`) and refreshes `codex-plugin` in place rather than overwriting Codex back to `[mcp_servers.apexuse]`.
- To switch back to direct TOML server config, run `npx -y apexuse-mcp install codex --write --npx`, which sets `enabled = false` on `[plugins."apexuse@apexuse-local"]` and writes `[mcp_servers.apexuse]`.

## 4. Live Probe & OS Permissions (`probe`, `doctor`, `onboard`)

```sh
# Standalone live MCP handshake probe (initialize -> notifications/initialized -> tools/list)
npx -y apexuse-mcp probe [--npx] [--policy <path>] [--timeout-ms 10000] [--mock]

# Guided macOS permission onboarding window (host-aware Accessibility + Screen Recording setup)
npx -y apexuse-mcp doctor --onboard
# or:
npx -y apexuse-mcp onboard [--force] [--no-auto-close] [--check]

# Headless readiness check across macOS, Linux, and Windows (never opens GUI)
npx -y apexuse-mcp doctor --check
```

## 5. Install Rich Agent Skill Pack (`npx skills add`)

To install the full `.agents/skills/apexuse/` pack (`SKILL.md`, `agents/openai.yaml`, and `references/`):

```sh
npx skills add Jackpkn/apexuse -g -a codex --skill apexuse -y
```
