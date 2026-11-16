import pytest
from types import SimpleNamespace
from handsmcp.munim_client import MunimClient

class Session:
    def __init__(self, text, error=False): self.text,self.error = text,error
    async def call_tool(self,name,args):
        return SimpleNamespace(content=[SimpleNamespace(type='text',text=self.text)],isError=self.error)

@pytest.mark.anyio
async def test_permission_error_is_not_reported_as_success():
    result = await MunimClient(Session('error: access denied', True)).call('list_apps', {})
    assert result['ok'] is False and result['code'] == 'upstream_error'

@pytest.mark.anyio
async def test_empty_response_fails_closed():
    result = await MunimClient(Session('')).call('list_apps', {})
    assert result['ok'] is False

@pytest.fixture
def anyio_backend(): return 'asyncio'

@pytest.mark.anyio
async def test_out_of_range_window_is_reported_as_error():
    response = 'Google Chrome [com.google.Chrome] pid=123\n\n(window 2 is out of range)'
    result = await MunimClient(Session(response)).call('get_app_state', {'window': 2})
    assert result['ok'] is False and result['code'] == 'window_not_found'

@pytest.mark.anyio
async def test_capture_preserves_mcp_image_and_rejects_errors():
    from mcp.types import ImageContent, TextContent
    class ImageSession:
        async def call_tool(self, name, args):
            assert name == 'screenshot' and args == {'app':'bundle:com.example.app'}
            return SimpleNamespace(content=[TextContent(type='text',text='window capture'), ImageContent(type='image',data='aGVsbG8=',mimeType='image/png')],isError=False)
    result=await MunimClient(ImageSession()).capture({'app':'bundle:com.example.app'})
    assert result['ok'] and result['image'].data == 'aGVsbG8=' and result['image'].mimeType == 'image/png'
    result=await MunimClient(Session('error: Screen Recording permission denied', True)).capture({'app':'bundle:com.example.app'})
    assert result['ok'] is False and result['code']=='upstream_error' and 'image' not in result

@pytest.mark.anyio
async def test_capture_rejects_missing_or_multiple_images():
    class ImageSession:
        async def call_tool(self, name, args):
            return SimpleNamespace(content=[SimpleNamespace(type='text',text='no image')],isError=False)
    result=await MunimClient(ImageSession()).capture({'app':'bundle:com.example.app'})
    assert result['ok'] is False and result['code']=='invalid_upstream_response'
