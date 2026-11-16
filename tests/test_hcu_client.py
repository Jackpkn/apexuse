import pytest
from types import SimpleNamespace
from handsmcp.hcu_client import HCUClient

class Session:
    def __init__(self, text, error=False):
        self.text, self.error = text, error
    async def call_tool(self, name, args):
        return SimpleNamespace(content=[SimpleNamespace(type='text', text=self.text)], isError=self.error)

@pytest.mark.anyio
async def test_preserves_upstream_failure_and_input_evidence():
    client = HCUClient(Session('{"ok":false,"code":"stale_ref","input_emitted":false}'))
    result = await client.call('AX', {})
    assert result['code'] == 'stale_ref'
    assert result['input_emitted'] is False
    assert result['ok'] is False

@pytest.mark.anyio
async def test_invalid_upstream_status_fails_closed():
    client = HCUClient(Session('not a JSON status'))
    assert (await client.call('AX', {}))['code'] == 'invalid_upstream_response'

@pytest.fixture
def anyio_backend():
    return 'asyncio'
