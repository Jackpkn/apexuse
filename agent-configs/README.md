# Agent Configuration Guide

`apexuse-mcp` runs as a local stdio MCP server with zero external runtime dependencies, no network listener, and no external LLM API keys.

## 1. Zero-Config Install via `npx` (Recommended)

Run via `npx` (Node.js `>=18`) to auto-wire detected AI coding agents safely (creates `.bak` backups of existing configs, validates JSON/TOML before writing, installs the bundled `SKILL.md`, and probes the live MCP handshake):

```sh
npx -y apexuse-mcp install all --write --npx --with-skill --probe
```

Or wire a single client (`antigravity`, `gemini`, `claude`, `claude-desktop`, `codex`, `codex-plugin`, `cursor`, `windsurf`, `vscode`, or `opencode`):

```sh
# Standard MCP client config + live probe
npx -y apexuse-mcp install cursor --write --npx --with-skill --probe

# Codex Desktop App Plugin (materializes ~/.codex/plugins/ & updates ~/.codex/config.toml)
npx -y apexuse-mcp install codex-plugin --write --npx --probe

# macOS guided permission onboarding UI (host-aware Accessibility + Screen Recording)
npx -y apexuse-mcp doctor --onboard

# Cross-platform readiness check & standalone live MCP probe
npx -y apexuse-mcp doctor --check
npx -y apexuse-mcp probe --npx
```

When `[plugins."apexuse@apexuse-local"]` (`enabled = true`) is active in `~/.codex/config.toml`, `install all --write` preserves and refreshes `codex-plugin` in place instead of overwriting it with `[mcp_servers.apexuse]`. Running `install codex --write` switches back cleanly by setting `enabled = false` on the plugin entry and writing `[mcp_servers.apexuse]`.

### Useful `install` Flags

- `--write` — safely merge `apexuse` into the target config file (backing up non-empty files to `<target>.bak`)
- `--dry-run` — preview the merged config and target path without modifying disk
- `--scope <user|project>` — write user-global config (default) or workspace/project config (`claude-desktop` and `codex-plugin` support `user` only)
- `--npx` — write `"command": "npx", "args": ["-y", "apexuse-mcp"]` instead of an absolute binary path
- `--policy <path>` — inject `APEXUSE_POLICY=<path>` into the client's `env` / `environment` block
- `--with-skill` — copy the bundled `SKILL.md` into standard agent skill directories (`~/.agents/skills/apexuse/SKILL.md`, `~/.claude/skills/apexuse/SKILL.md`, `~/.codex/skills/apexuse/SKILL.md`, or `.agents/` / `.claude/` in `--scope project`)
- `--probe` — spawn the configured MCP invocation after install and verify JSON-RPC `initialize` + `tools/list` (`--timeout-ms <n>` defaults to `10000`; `--mock` applies only to the temporary probe subprocess and is never written to config files)

Pair the MCP server with [`.agents/skills/apexuse/SKILL.md`](../.agents/skills/apexuse/SKILL.md) (and its [`agents/openai.yaml`](../.agents/skills/apexuse/agents/openai.yaml) + [`references/`](../.agents/skills/apexuse/references/installation.md)), or install the full skill pack directly with:

```sh
npx skills add Jackpkn/apexuse -g -a codex --skill apexuse -y
```

---

## 2. Supported Clients & Config Locations

| Agent | User Scope (`--scope user`) | Project Scope (`--scope project`) | Config Key / Format |
| --- | --- | --- | --- |
| `antigravity` | `~/.gemini/antigravity/mcp_config.json` | `.agents/mcp_config.json` | JSON `"mcpServers"` |
| `gemini` | `~/.gemini/settings.json` | `.gemini/settings.json` | JSON `"mcpServers"` |
| `claude` | `~/.claude.json` | `.mcp.json` | JSON `"mcpServers"` (also prints `claude mcp add` hint) |
| `claude-desktop` | macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`<br>Windows: `%APPDATA%\Claude\claude_desktop_config.json`<br>Linux: `~/.config/Claude/claude_desktop_config.json` | *(unsupported)* | JSON `"mcpServers"` |
| `codex` | `~/.codex/config.toml` | `.codex/config.toml` | TOML `[mcp_servers.apexuse]` |
| `codex-plugin` | `~/.codex/config.toml` + `~/.codex/plugins/{marketplaces/apexuse-local, cache/apexuse-local/apexuse/0.3.0}` | *(unsupported)* | TOML `[marketplaces.apexuse-local]` + `[plugins."apexuse@apexuse-local"]` + `.mcp.json` |
| `cursor` | `~/.cursor/mcp.json` | `.cursor/mcp.json` | JSON `"mcpServers"` |
| `windsurf` | `~/.codeium/windsurf/mcp_config.json` | `.codeium/windsurf/mcp_config.json` | JSON `"mcpServers"` |
| `vscode` | macOS: `~/Library/Application Support/Code/User/mcp.json`<br>Windows: `%APPDATA%\Code\User/mcp.json`<br>Linux: `~/.config/Code/User/mcp.json` | `.vscode/mcp.json` | JSON `"servers"` (`"type": "stdio"`) |
| `opencode` | `~/.config/opencode/opencode.json` (`$XDG_CONFIG_HOME/opencode/opencode.json`) | `opencode.json` | JSON `"mcp"` (`"type": "local"`, `"enabled": true`) |

---

## 3. Copy-Paste Manual Snippets

### Antigravity / Gemini CLI / Cursor / Windsurf / Claude Desktop

Merge into the client's `"mcpServers"` object:

```json
{
  "mcpServers": {
    "apexuse": {
      "command": "npx",
      "args": ["-y", "apexuse-mcp"],
      "env": {
        "APEXUSE_POLICY": "/absolute/path/to/repo/policy.autonomous.yaml"
      }
    }
  }
}
```

### Claude Code

```sh
claude mcp add apexuse -- npx -y apexuse-mcp
```

### OpenAI Codex (`~/.codex/config.toml`)

Direct MCP server entry:

```toml
[mcp_servers.apexuse]
command = "npx"
args = ["-y", "apexuse-mcp"]
startup_timeout_sec = 60
tool_timeout_sec = 180
```

Or install the Codex Desktop Plugin automatically (recommended for Codex Desktop App):

```sh
npx -y apexuse-mcp install codex-plugin --write --npx --probe
```

### VS Code (`mcp.json`)

```json
{
  "servers": {
    "apexuse": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "apexuse-mcp"]
    }
  }
}
```

### OpenCode (`opencode.json`)

```json
{
  "mcp": {
    "apexuse": {
      "type": "local",
      "command": ["npx", "-y", "apexuse-mcp"],
      "enabled": true
    }
  }
}
```

---

## 4. Persistent Runtime Defaults & Direct CLI Calls

Customize defaults in `~/.apexuse/config.json` (or `APEXUSE_CONFIG`):

```sh
npx -y apexuse-mcp config show
npx -y apexuse-mcp config set image.format png
npx -y apexuse-mcp config set image.max_width 1600
npx -y apexuse-mcp config set snapshot.reuse_form_refs true
npx -y apexuse-mcp config set snapshot.ax_diff true
npx -y apexuse-mcp config set snapshot.include_bounds false
npx -y apexuse-mcp config reset
```

Run one-off or stateful multi-step calls directly from the shell:

```sh
npx -y apexuse-mcp call inspect_app --args '{"app":"Calculator"}'
npx -y apexuse-mcp call visual_inspect --args '{"app":"Calculator","format":"jpeg"}' --save-image out.jpg
npx -y apexuse-mcp call --calls '[{"tool":"inspect_app","args":{"app":"Calculator"}},{"tool":"computer","args":{"action":"click","arguments":{"target":{"query":"7"},"return_state":"diff","verify_change":true,"verify_mode":"ax"}}}]'
```
