# Agent Configuration Guide

`apexuse-mcp` runs as a local stdio MCP server with no network listener or external LLM dependency.

First build the release binary:

```sh
cargo build --release --manifest-path /absolute/path/to/repo/rust/Cargo.toml
```

You can also print the snippet for your client directly from the binary:

```sh
/absolute/path/to/repo/rust/target/release/apexuse-mcp install antigravity
```

Pair the MCP server with [`.agents/skills/apexuse/SKILL.md`](../.agents/skills/apexuse/SKILL.md) so your agent follows optimal `inspect_app` / `visual_inspect` / `run_actions` patterns.

---

## Antigravity IDE or CLI

Add to `~/.gemini/antigravity/mcp_config.json` (global) or workspace `.agents/mcp_config.json`:

```json
{
  "mcpServers": {
    "apexuse": {
      "command": "/absolute/path/to/repo/rust/target/release/apexuse-mcp",
      "args": [],
      "env": {
        "APEXUSE_POLICY": "/absolute/path/to/repo/policy.autonomous.yaml"
      }
    }
  }
}
```

## Gemini CLI

Merge the same `mcpServers` block into `~/.gemini/settings.json` (or workspace `.gemini/settings.json`). Keep any existing `mcpServers` entries intact.

## OpenAI Codex

Add to your Codex config:

```toml
[mcp_servers.apexuse]
command = "/absolute/path/to/repo/rust/target/release/apexuse-mcp"
args = []
startup_timeout_sec = 60
tool_timeout_sec = 180

[mcp_servers.apexuse.env]
APEXUSE_POLICY = "/absolute/path/to/repo/policy.autonomous.yaml"
```

## Claude Code

```sh
claude mcp add apexuse --env APEXUSE_POLICY=/absolute/path/to/repo/policy.autonomous.yaml -- /absolute/path/to/repo/rust/target/release/apexuse-mcp
```
