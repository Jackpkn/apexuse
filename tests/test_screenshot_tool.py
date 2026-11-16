import pytest
from types import SimpleNamespace
from mcp.types import ImageContent
from handsmcp.server import screenshot

@pytest.fixture
def anyio_backend(): return 'asyncio'

@pytest.mark.anyio
async def test_tool_returns_image_blocks_and_reports_permission_failure():
    class Gateway:
        async def capture_image(self, app):
            if app == 'denied': return {'ok':False,'code':'upstream_error','message':'Screen Recording denied'}
            return {'ok':True,'message':'captured','image':ImageContent(type='image',data='aGVsbG8=',mimeType='image/png')}
    ctx=SimpleNamespace(request_context=SimpleNamespace(lifespan_context={'gateway':Gateway()}))
    result=await screenshot('com.example.app',ctx)
    assert [item.type for item in result]==['text','image']
    result=await screenshot('denied',ctx)
    assert len(result)==1 and 'Screen Recording denied' in result[0].text
