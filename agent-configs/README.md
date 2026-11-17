# Agent Configuration Guide

`apexuse-mcp` runs as a local stdio MCP server with zero external runtime dependencies, no network listener, and no external LLM API keys.

## 1. Zero-Config Install via `npx` (Recommended)

Run via `npx` (Node.js `>=18`) to auto-wire any installed AI coding agent safely (creates `.bak` backups of existing configs, validates JSON/TOML before writing, and installs the bundled `SKILL.md`):

```sh
npx -y apexuse-mcp install all --write --npx --with-skill
```

Or wire a single client (`antigravity`, `gemini`, `claude`, `claude-desktop`, `codex`, `cursor`, `windsurf`, `vscode`, or `opencode`):

```sh
npx -y apexuse-mcp install cursor --write --npx --with-skill
npx -y apexuse-mcp doctor --check
```

### Useful `install` Flags

- `--write` — safely merge `apexuse` into the target config file (backing up non-empty files to `<target>.bak`)
- `--dry-run` — preview the merged config and target path without modifying disk
- `--scope <user|project>` — write user-global config (default) or workspace/project config (note: `claude-desktop` supports `user` only)
- `--npx` — write `"command": "npx", "args": ["-y", "apexuse-mcp"]` instead of an absolute binary path
- `--policy <path>` — inject `APEXUSE_POLICY=<path>` into the client's `env` / `environment` block
- `--with-skill` — copy the bundled `SKILL.md` into standard agent skill directories (`~/.agents/skills/apexuse/SKILL.md`, `~/.claude/skills/apexuse/SKILL.md`, `~/.codex/skills/apexuse/SKILL.md`, or `.agents/` / `.claude/` in `--scope project`)

Pair the MCP server with [`.agents/skills/apexuse/SKILL.md`](../.agents/skills/apexuse/SKILL.md) so your agent uses `inspect_app`, `set_value`, `select_text`, semantic `target`, `keep_snapshot`, and batched `run_actions` effectively.

---

## 2. Supported Clients & Config Locations

| Agent | User Scope (`--scope user`) | Project Scope (`--scope project`) | Config Key / Format |
| --- | --- | --- | --- |
| `antigravity` | `~/.gemini/antigravity/mcp_config.json` | `.agents/mcp_config.json` | JSON `"mcpServers"` |
| `gemini` | `~/.gemini/settings.json` | `.gemini/settings.json` | JSON `"mcpServers"` |
| `claude` | `~/.claude.json` | `.mcp.json` | JSON `"mcpServers"` (also prints `claude mcp add` hint) |
| `claude-desktop` | macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`<br>Windows: `%APPDATA%\Claude\claude_desktop_config.json`<br>Linux: `~/.config/Claude/claude_desktop_config.json` | *(unsupported)* | JSON `"mcpServers"` |
| `codex` | `~/.codex/config.toml` | `.codex/config.toml` | TOML `[mcp_servers.apexuse]` |
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

```toml
[mcp_servers.apexuse]
command = "npx"
args = ["-y", "apexuse-mcp"]
startup_timeout_sec = 60
tool_timeout_sec = 180
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
npx -y apexuse-mcp config reset
```

Run one-off or stateful multi-step calls directly from the shell:

```sh
npx -y apexuse-mcp call inspect_app --args '{"app":"Calculator"}'
npx -y apexuse-mcp call visual_inspect --args '{"app":"Calculator","format":"jpeg"}' --save-image out.jpg
npx -y apexuse-mcp call --calls '[{"tool":"inspect_app","args":{"app":"Calculator"}},{"tool":"computer","args":{"action":"click","arguments":{"target":{"query":"7"}}}}]'
```
