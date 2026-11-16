# Experimental agent connection

The server is incomplete. These examples only connect the current limited prototype; they do not provide general computer control. Keep the agent's per-tool approvals enabled, and install or provide `skills/handsmcp/SKILL.md` to the agent.

Set the absolute project path below. The Python environment and built `runtime/munim-computer-use` must already exist. The Cua preview has been exercised in Antigravity for application listing and inspection; the other clients and general desktop control remain unverified.

## Antigravity IDE or CLI

Use MCP Servers > Manage MCP Servers > View raw config in the IDE, or the CLI's MCP manager. Official documentation describes the global `~/.gemini/config/mcp_config.json` and workspace `.agents/mcp_config.json`: https://antigravity.google/docs/mcp

```json
{"mcpServers":{"handsmcp":{"command":"/absolute/path/handsmcp/.venv/bin/python","args":["-m","handsmcp.server"],"env":{"PYTHONPATH":"/absolute/path/handsmcp/src"}}}}
```

## Gemini CLI

The same `mcpServers` entry belongs in `~/.gemini/settings.json`; see https://geminicli.com/docs/tools/mcp-server/. Do not overwrite existing settings.

## Codex

```toml
[mcp_servers.handsmcp]
command = "/absolute/path/handsmcp/.venv/bin/python"
args = ["-m", "handsmcp.server"]
startup_timeout_sec = 60
tool_timeout_sec = 180
[mcp_servers.handsmcp.env]
PYTHONPATH = "/absolute/path/handsmcp/src"
```

## Claude Code

```sh
claude mcp add handsmcp --env PYTHONPATH=/absolute/path/handsmcp/src -- /absolute/path/handsmcp/.venv/bin/python -m handsmcp.server
```

The upstream HCU Claude schema adapter applies to HCU's AX/GUI schemas; whether this server's own schema needs adaptation has not been tested.
