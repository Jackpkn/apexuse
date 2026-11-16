"""Adapter for the narrowly permitted Munim application inventory."""
import base64
import binascii
from mcp.types import ImageContent

class MunimClient:
    def __init__(self, session):
        self.session = session

    async def capture(self, arguments):
        response = await self.session.call_tool('screenshot', arguments)
        text = '\n'.join(block.text for block in response.content if block.type == 'text')
        if response.isError or text.lstrip().lower().startswith('error:'):
            return {'ok':False, 'code':'upstream_error', 'message':text[:500]}
        images = [block for block in response.content if isinstance(block, ImageContent)]
        if len(images) != 1 or images[0].mimeType not in {'image/png','image/jpeg'} or len(images[0].data) > 20_000_000:
            return {'ok':False, 'code':'invalid_upstream_response'}
        try:
            if not base64.b64decode(images[0].data, validate=True):
                raise ValueError('empty image')
        except (ValueError, binascii.Error):
            return {'ok':False, 'code':'invalid_upstream_response'}
        return {'ok':True, 'image':images[0], 'message':text[:500]}

    async def call(self, name, arguments):
        response = await self.session.call_tool(name, arguments)
        parts = [block.text for block in response.content if block.type == 'text']
        content = '\n'.join(parts)
        if response.isError or content.lstrip().lower().startswith('error:'):
            return {'ok': False, 'code': 'upstream_error', 'content': content}
        if (name == 'get_app_state' and type(arguments.get('window')) is int
                and content.rstrip().endswith(f"\n\n(window {arguments['window']} is out of range)")):
            return {'ok': False, 'code': 'window_not_found', 'content': ''}
        if not content.strip():
            return {'ok': False, 'code': 'invalid_upstream_response', 'content': ''}
        return {'ok': True, 'content': content}
