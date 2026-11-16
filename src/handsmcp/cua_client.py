"""Cua adapter. Only inventory is implemented; all other calls fail closed."""
import json


class CuaClient:
    def __init__(self, session):
        self.session = session

    async def call(self, name, arguments):
        if name == 'get_app_state':
            return await self.inspect(arguments)
        if name != 'list_apps' or arguments != {}:
            return {'ok': False, 'code': 'not_implemented', 'content': ''}
        try:
            response = await self.session.call_tool('list_apps', {})
            data = response.structuredContent
            if response.isError or not isinstance(data, dict) or 'refusal' in data:
                raise ValueError('refused')
            apps = data.get('apps')
            if not isinstance(apps, list) or len(apps) > 10000:
                raise ValueError('invalid app inventory')
            records = []
            for app in apps:
                if not isinstance(app, dict):
                    raise ValueError('invalid app')
                if app.get('running') is not True:
                    continue
                bundle, pid = app.get('bundle_id'), app.get('pid')
                if not isinstance(bundle, str) or not bundle or type(pid) is not int or pid <= 0:
                    raise ValueError('invalid running app')
                records.append({'bundle_id': bundle, 'pid': pid})
            return {'ok': True, 'content': json.dumps(records)}
        except Exception:
            return {'ok': False, 'code': 'upstream_error', 'content': ''}

    async def inspect(self, arguments):
        from .settings import valid_bundle
        if not isinstance(arguments, dict):
            return {'ok': False, 'code': 'invalid_request', 'content': ''}
        app = arguments.get('app', '')
        if not isinstance(app, str) or not app.startswith('bundle:') or not valid_bundle(app[7:]):
            return {'ok': False, 'code': 'invalid_request', 'content': ''}
        bundle = app[7:]
        try:
            inventory = await self.call('list_apps', {})
            if not inventory.get('ok'):
                raise ValueError('inventory unavailable')
            matches = [a for a in json.loads(inventory['content']) if a['bundle_id'] == bundle]
            if len(matches) != 1:
                raise ValueError('ambiguous app')
            pid = matches[0]['pid']
            result = await self.session.call_tool('list_windows', {'pid': pid})
            data = result.structuredContent
            if result.isError or not isinstance(data, dict) or 'refusal' in data:
                raise ValueError('windows unavailable')
            windows = [w for w in data.get('windows', []) if w.get('pid') == pid and w.get('layer', 0) == 0]
            if not windows or len(windows) > 20 or any(type(w.get('window_id')) is not int for w in windows):
                raise ValueError('windows unavailable')
            selected = range(len(windows))
            if 'window' in arguments:
                index = arguments['window']
                if type(index) is not int or not 0 <= index < len(windows):
                    return {'ok':False, 'code':'window_not_found', 'content':''}
                selected = [index]
            snapshots = []
            for index in selected:
                wid = windows[index]['window_id']
                response = await self.session.call_tool('get_window_state', {'pid':pid,'window_id':wid,'include_screenshot':False,'tree_format':'elements','full_output':True})
                state = response.structuredContent
                if response.isError or not isinstance(state, dict) or state.get('pid') != pid or state.get('window_id') != wid or not safe_snapshot(state):
                    return {'ok':False, 'code':'unsafe_or_incomplete_tree', 'content':''}
                snapshots.append((index, state))
            lines = [f'Application [{bundle}] pid={pid} windows={len(windows)}']
            ref = 0
            for index, state in snapshots:
                lines.append(f'── window {index}:')
                for element in state['elements']:
                    role = element['role']
                    if not role.startswith('AX') or not role[2:].isalnum():
                        continue
                    label = element.get('label') or element.get('description') or ''
                    if not isinstance(label, str):
                        raise ValueError('invalid label')
                    query = arguments.get('query')
                    if query is not None and (not isinstance(query, str) or query.casefold() not in label.casefold()):
                        continue
                    ref += 1
                    lines.append(f'  {role[2:]} {json.dumps(label, ensure_ascii=True)}')
            content = '\n'.join(lines)
            return {'ok':True, 'content':content}
        except Exception:
            return {'ok':False, 'code':'upstream_error', 'content':''}

    async def capture(self, arguments):
        return {'ok': False, 'code': 'not_implemented'}


def safe_snapshot(data):
    """Require positive completeness and reject secure or ambiguous fields."""
    import re
    if not isinstance(data, dict) or data.get('elements_complete') is not True or data.get('truncated') is not False:
        return False
    elements = data.get('elements')
    if not isinstance(elements, list) or len(elements) > 2000:
        return False
    marker = re.compile(r'secure.?text.?field|password|protected.?content|[\u2022\u25cf\uf79a]', re.I)
    for element in elements:
        if not isinstance(element, dict) or not isinstance(element.get('role'), str):
            return False
        for name in ('role', 'subrole', 'label', 'description', 'value'):
            value = element.get(name)
            if value is not None and not isinstance(value, (str, int, float, bool)):
                return False
            if isinstance(value, str) and marker.search(value):
                return False
    return True
