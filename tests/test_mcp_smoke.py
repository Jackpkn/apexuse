import json
import os
from pathlib import Path
import sys
import pytest
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

ROOT = Path(__file__).resolve().parents[1]

@pytest.mark.anyio
async def test_stdio_tools_and_denied_mutation():
    if not (ROOT / 'runtime/munim-computer-use').is_file():
        pytest.skip('Munim macOS runtime not installed')
    env = dict(os.environ, PYTHONPATH=str(ROOT / 'src'), HANDSMCP_MUNIM=str(ROOT / 'runtime/munim-computer-use'))
    async with stdio_client(StdioServerParameters(command=sys.executable, args=['-m', 'handsmcp.server'], env=env)) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools = await session.list_tools()
            assert {tool.name for tool in tools.tools} == {'computer', 'desktop', 'screenshot', 'inspect_app'}
            result = await session.call_tool('computer', {'action': 'click', 'arguments': {'ref': 'r1'}})
            payload = result.structuredContent
            assert isinstance(payload, dict)
            assert payload['code'] == 'not_implemented'
            assert payload['input_emitted'] is False

@pytest.fixture
def anyio_backend():
    return 'asyncio'

@pytest.mark.anyio
async def test_inspect_app_has_read_only_metadata():
    from handsmcp.server import mcp
    listed = {item.name: item for item in await mcp.list_tools()}
    assert listed['inspect_app'].annotations.readOnlyHint is True

@pytest.mark.anyio
async def test_inspect_app_uses_existing_gateway_policy():
    from types import SimpleNamespace
    from handsmcp.server import inspect_app
    class Gateway:
        async def dispatch(self, tool, action, args): return {'tool':tool,'action':action,'args':args}
    ctx=SimpleNamespace(request_context=SimpleNamespace(lifespan_context={'gateway':Gateway()}))
    result=await inspect_app('com.google.Chrome',ctx,window=0,query='Apply')
    assert result == {'tool':'computer','action':'inspect','args':{'app':'com.google.Chrome','window':0,'query':'Apply'}}
