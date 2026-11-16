"""Experimental stdio MCP server; desktop mutations are disabled."""
from contextlib import asynccontextmanager
import os
import shutil
from pathlib import Path
from typing import Any
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.server.fastmcp import FastMCP, Context
from mcp.types import TextContent, ImageContent, ToolAnnotations
from .gateway import Gateway
from .audit import AuditLog
from .desktop_lock import DesktopLock
from .settings import load_settings
from .munim_client import MunimClient
from .cua_client import CuaClient
from .hybrid_client import HybridClient

def child_parameters(runtime):
    return StdioServerParameters(command=runtime, args=[], env={"COMPUTER_USE_ALLOW_SECURE_FIELD_INPUT": "0", "HANDSMCP_SAFE_INVENTORY": "1", "HANDSMCP_RESTRICT_TARGETS": "1", "HANDSMCP_DISABLE_BROWSER_BRIDGE": "1", "COMPUTER_USE_REMOTE_CONTROL": "0", "COMPUTER_USE_AGENT_CURSOR": "1", "COMPUTER_USE_AGENT_CURSOR_TASK_FADE_SECS": "8"})

def backend_parameters(munim_runtime):
    backend = os.environ.get('HANDSMCP_BACKEND', 'munim')
    if backend == 'munim':
        return backend, child_parameters(munim_runtime)
    if backend == 'cua':
        runtime = os.environ.get('HANDSMCP_CUA') or shutil.which('cua-driver') or '/Applications/CuaDriver.app/Contents/MacOS/cua-driver'
        return backend, StdioServerParameters(command=runtime, args=['mcp'], env={'DO_NOT_TRACK': '1'})
    raise ValueError('unknown HANDSMCP_BACKEND')

@asynccontextmanager
async def lifespan(server):
    root = Path(__file__).resolve().parents[2]
    settings = load_settings(os.environ.get('HANDSMCP_POLICY', str(root / 'policy.yaml')))
    runtime = os.environ.get('HANDSMCP_MUNIM', str(Path(__file__).resolve().parents[2] / 'runtime' / 'munim-computer-use'))
    backend, params = backend_parameters(runtime)
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            if backend == 'cua':
                async with stdio_client(child_parameters(runtime)) as (mread, mwrite):
                    async with ClientSession(mread, mwrite) as msession:
                        await msession.initialize()
                        yield {'gateway': make_gateway(HybridClient(MunimClient(msession), session), settings)}
            else:
                yield {'gateway': make_gateway(MunimClient(session), settings)}


def make_gateway(backend, settings):
    return Gateway(backend, audit=AuditLog(Path.home() / '.handsmcp/logs/audit.jsonl'), stop_file=Path.home() / '.handsmcp/STOP', desktop_lock=DesktopLock(Path.home() / '.handsmcp/desktop.lock'), mode=settings.mode, rate_limit=settings.rate_limit_per_minute, confirmation_ttl=settings.confirmation_ttl_seconds, allowed_apps=settings.allowed_apps)

mcp = FastMCP('handsmcp', lifespan=lifespan)

@mcp.tool(structured_output=True, annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False, openWorldHint=False))
async def inspect_app(app: str, ctx: Context, window: int | None = None, query: str | None = None) -> dict[str, Any]:
    """Inspect an allowed local app. Returns a fresh accessibility snapshot; no input is sent."""
    args: dict[str, Any] = {'app': app}
    if window is not None: args['window'] = window
    if query is not None: args['query'] = query
    return await ctx.request_context.lifespan_context['gateway'].dispatch('computer', 'inspect', args)

@mcp.tool(structured_output=True)
async def computer(action: str, arguments: dict, ctx: Context) -> dict[str, Any]:
    """Inspect with action=inspect and arguments={"app":"Calculator"}, {"app":"TextEdit"}, or an exact bundle ID permitted by policy.yaml. Optionally add a zero-based window index and a query string to search the accessibility tree. Click an identified Button with arguments={"ref":"e1"}; type with arguments={"ref":"e1","text":"hello"} using a current TextArea, verified TextField or SearchField ref; add mode=replace to replace the field (up to 2000 characters; insert is limited to 200). Unverified and secure fields are blocked. Following an unfiltered inspection, press_key accepts navigation keys and Command/Control shortcuts, e.g. {"key":"s","modifiers":["cmd"]}; scroll accepts {"direction":"down","amount":4}. Ordinary characters are not accepted through press_key. If confirmation_required, present the exact summary to the user, wait for explicit approval, then call action=confirm with arguments={"token":"..."}. Never confirm without approval."""
    return await ctx.request_context.lifespan_context['gateway'].dispatch('computer', action, arguments)

@mcp.tool(structured_output=True)
async def desktop(action: str, arguments: dict, ctx: Context) -> dict[str, Any]:
    """List applications with action=list_applications and arguments={}. Launch or activate an allowed app with action=launch_application or activate_application and arguments={"app":"exact.bundle.id"}; inspect afterward to verify it opened. After a complete unfiltered inspection of all windows, visual_click accepts {"x":120,"y":200} in screen points. It requires confirmation and a fresh, unchanged snapshot; verify the result. Window listing and other operations are disabled."""
    return await ctx.request_context.lifespan_context['gateway'].dispatch('desktop', action, arguments)

@mcp.tool(structured_output=False)
async def screenshot(app: str, ctx: Context) -> list[TextContent | ImageContent]:
    """Capture the largest window of an allowed running app by exact bundle ID. All app windows must have complete, non-protected accessibility trees; check the returned metadata to identify the captured window. Screen Recording permission is required. Capturing refreshes element refs; inspect again before acting."""
    result = await ctx.request_context.lifespan_context['gateway'].capture_image(app)
    if not result.get('ok'):
        return [TextContent(type='text',text=f"Screenshot failed: {result.get('code','unknown')}. {result.get('message','')}")]
    return [TextContent(type='text',text=result.get('message') or 'Screenshot captured.'), result['image']]

def main():
    mcp.run(transport='stdio')

if __name__ == '__main__':
    main()
