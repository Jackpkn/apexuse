"""Translate a child MCP result without confusing input delivery with success."""
import json

class HCUClient:
    def __init__(self, session):
        self.session = session

    async def call(self, name, arguments):
        response = await self.session.call_tool(name, arguments)
        content = [block.text for block in response.content if block.type == 'text']
        if not content:
            return {'ok': False, 'code': 'invalid_upstream_response', 'content': ''}
        raw = '\n'.join(content)
        try:
            status = json.loads(content[0].split('\n', 1)[0])
        except (ValueError, TypeError):
            return {'ok': False, 'code': 'invalid_upstream_response', 'content': raw}
        if not isinstance(status, dict) or not isinstance(status.get('ok'), bool):
            return {'ok': False, 'code': 'invalid_upstream_response', 'content': raw}
        if response.isError:
            status['ok'] = False
        status['content'] = raw
        return status
