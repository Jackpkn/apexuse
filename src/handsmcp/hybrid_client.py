"""Experimental Cua click bridge with the existing Munim safety preflight."""
import json
import math
import re
import uuid
from .gateway import Gateway, contains_secure_marker
from .settings import valid_bundle


class HybridClient:
    def __init__(self, munim, cua):
        self.munim, self.cua = munim, cua
        self.session = "handsmcp-" + uuid.uuid4().hex
        self.cursor_session = self.session + "-cursor"
        self.raw = None
        self.inspect_args = None
        self.preflight = Gateway(munim)

    async def capture(self, arguments):
        return await self.munim.capture(arguments)

    async def call(self, name, arguments):
        if name == 'get_app_state':
            result = await self.munim.call(name, arguments)
            self.raw = result.get('content') if result.get('ok') else None
            self.inspect_args = dict(arguments) if self.raw else None
            return result
        if name != 'click':
            return await self.munim.call(name, arguments)
        if set(arguments) == {'app', 'x', 'y'}:
            app = arguments.get('app')
            if (not isinstance(app, str) or not app.startswith('bundle:')
                    or not valid_bundle(app[7:]) or not self.inspect_args or not self.raw
                    or self.inspect_args.get('app') != app
                    or any(type(arguments[k]) not in (int, float) or not math.isfinite(arguments[k]) for k in ('x', 'y'))
                    or not await self.preflight._complete_visual_preflight(self.raw, app[7:], self.inspect_args)):
                return {'ok': False, 'code': 'unsafe_or_ambiguous_target', 'content': ''}
            # The pointer is visual only; native input still performs its own
            # window-bounds and hit checks after the gateway's confirmation.
            if not await self._show_cursor(arguments['x'], arguments['y']):
                return {'ok': False, 'code': 'cursor_unavailable', 'content': ''}
            return await self.munim.call(name, arguments)
        return await self._click(arguments)

    async def _show_cursor(self, x, y):
        steps = (
            ('set_agent_cursor_enabled', {'session': self.cursor_session, 'enabled': True}),
            ('set_agent_cursor_motion', {'session': self.cursor_session, 'idle_hide_ms': 2000, 'timing': 'steady'}),
            ('move_cursor', {'session': self.cursor_session, 'x': x, 'y': y}),
        )
        try:
            for tool, params in steps:
                result = await self.cua.call_tool(tool, params)
                data = result.structuredContent
                if result.isError:
                    return False
                if tool == 'move_cursor' and data is None:
                    expected = f"Agent cursor '{self.cursor_session}' moved to ({x:.1f}, {y:.1f})."
                    content = getattr(result, 'content', None)
                    if not isinstance(content, list) or len(content) != 1 or getattr(content[0], 'type', None) != 'text' or getattr(content[0], 'text', None) != expected:
                        return False
                elif not isinstance(data, dict) or 'refusal' in data or data.get('status') == 'refused':
                    return False
            return True
        except Exception:
            return False

    async def _click(self, args):
        denied = {'ok':False,'code':'unsafe_or_ambiguous_target','content':''}
        try:
            bundle=args.get('handsmcp_expected_bundle'); ref=args.get('element_id'); target=args.get('handsmcp_expected_name')
            if not valid_bundle(bundle) or not isinstance(target,str) or not isinstance(ref,str) or not self.inspect_args or not self.raw:
                return denied
            if self.inspect_args.get('app') != 'bundle:'+bundle:
                return denied
            if not await self.preflight._complete_visual_preflight(self.raw,bundle,self.inspect_args):
                return denied
            header=re.match(r'^[^\n]*\['+re.escape(bundle)+r'\]\s+pid=(\d+)\b[^\n]*\bwindows=1\b',self.raw)
            if not header: return denied
            buttons=re.findall(r'^\s*\[(e\d+)\]\s+Button "([^"\n]+)"(?:\s|$)',self.raw,re.M)
            if [(r,n) for r,n in buttons if n==target] != [(ref,target)]: return denied
            pid=int(header.group(1))
            windows_result=await self.cua.call_tool('list_windows',{'pid':pid})
            wd=windows_result.structuredContent
            if windows_result.isError or not isinstance(wd,dict) or 'refusal' in wd or not isinstance(wd.get('windows'),list): return denied
            windows=[w for w in wd['windows'] if isinstance(w,dict) and w.get('pid')==pid and w.get('layer',0)==0]
            if len(windows) != 1 and bundle == 'com.google.Chrome':
                title = re.search(r'^── window 0: "([^"\n]+)"$', self.raw, re.M)
                if not title: return denied
                ax_title = title.group(1)
                def matches_chrome_title(window):
                    candidate = window.get('title')
                    return (isinstance(candidate,str) and bool(candidate) and
                            bool(re.fullmatch(re.escape(candidate) + r' - Google Chrome(?: – [^\n]+)?', ax_title)))
                windows = [w for w in windows if matches_chrome_title(w)]
            if len(windows)!=1 or type(windows[0].get('window_id')) is not int: return denied
            wid=windows[0]['window_id']
            state_result=await self.cua.call_tool('get_window_state',{'pid':pid,'window_id':wid,'include_screenshot':False,'tree_format':'elements','full_output':True,'session':self.session})
            state=state_result.structuredContent
            if state_result.isError or not isinstance(state,dict) or state.get('pid')!=pid or state.get('window_id')!=wid or state.get('truncated') is not False or state.get('depth_truncated') is True or not isinstance(state.get('elements'),list): return denied
            if state.get('degraded_reason') or contains_secure_marker(json.dumps(state.get('elements', []), ensure_ascii=False)):
                return denied
            snapshot=state.get('snapshot_id')
            matches=[e for e in state['elements'] if isinstance(e,dict) and e.get('role')=='AXButton' and e.get('label')==target]
            if len(matches)!=1 or matches[0].get('enabled') is not True: return denied
            token=matches[0].get('element_token')
            if not isinstance(snapshot,str) or not isinstance(token,str) or not re.fullmatch(r's[0-9a-f]{8}:\d+',token) or not token.startswith(snapshot+':'): return denied
            frame = matches[0].get('frame')
            if not isinstance(frame,dict) or any(type(frame.get(k)) not in (int,float) or not math.isfinite(frame[k]) for k in ('x','y','w','h')):
                return denied
            if frame['w'] <= 0 or frame['h'] <= 0 or any(abs(frame[k]) > 100000 for k in ('x','y','w','h')):
                return denied
            cursor_steps = (
                # Keep the visual cursor independent of Cua's target-pinned action cursor.
                ('set_agent_cursor_enabled', {'session':self.session, 'enabled':False}),
                ('set_agent_cursor_enabled', {'session':self.cursor_session, 'enabled':True}),
                ('set_agent_cursor_motion', {'session':self.cursor_session, 'idle_hide_ms':2000, 'timing':'steady'}),
                ('move_cursor', {'session':self.cursor_session, 'x':frame['x']+frame['w']/2, 'y':frame['y']+frame['h']/2}),
            )
            for tool, params in cursor_steps:
                cursor = await self.cua.call_tool(tool,params)
                data = cursor.structuredContent
                if cursor.isError: return denied
                if tool == 'move_cursor' and data is None:
                    # Cua returns this operation as text, unlike its other cursor tools.
                    expected = f"Agent cursor '{self.cursor_session}' moved to ({params['x']:.1f}, {params['y']:.1f})."
                    content = getattr(cursor, 'content', None)
                    if not isinstance(content, list) or len(content) != 1 or getattr(content[0], 'type', None) != 'text' or getattr(content[0], 'text', None) != expected: return denied
                elif not isinstance(data,dict) or 'refusal' in data or data.get('status')=='refused': return denied
            result=await self.cua.call_tool('click',{'element_token':token,'session':self.session})
            data=result.structuredContent
            if result.isError or not isinstance(data,dict) or 'refusal' in data or data.get('status')=='refused': return denied
            # The action may pin Cua's overlay behind a foreground window. Re-raise
            # the separate, unpinned pointer without turning a dispatched click
            # into a failure that could cause a duplicate click.
            try:
                await self.cua.call_tool('move_cursor', cursor_steps[-1][1])
            except Exception:
                pass
            return {'ok':True,'content':'Input dispatched; inspect the target to verify the result.'}
        except Exception:
            return denied
