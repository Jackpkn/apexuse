import asyncio
from types import SimpleNamespace
from handsmcp.hybrid_client import HybridClient

class Munim:
    def __init__(self, text): self.text=text
    async def call(self,name,args): return {'ok':True,'content':self.text}
class Cua:
    def __init__(self,elements): self.elements=[dict(e,frame=e.get('frame',{'x':100,'y':200,'w':40,'h':20})) for e in elements]; self.clicks=[]
    async def call_tool(self,name,args):
        if name=='list_windows': return SimpleNamespace(isError=False,structuredContent={'windows':[{'pid':123,'window_id':4,'layer':0}]})
        if name=='get_window_state': return SimpleNamespace(isError=False,structuredContent={'pid':123,'window_id':4,'snapshot_id':'s00000001','truncated':False,'elements':self.elements})
        if name in ('set_agent_cursor_enabled','set_agent_cursor_motion','move_cursor'): return SimpleNamespace(isError=False,structuredContent={'ok':True})
        if name=='click': self.clicks.append(args); return SimpleNamespace(isError=False,structuredContent={'effect':'unverifiable'})
        raise AssertionError(name)

def test_click_uses_unique_matched_button_after_inspection():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    async def run():
        c=Cua([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]); h=HybridClient(Munim(raw),c)
        assert (await h.call('get_app_state',{'app':'bundle:com.example.app'}))['ok']
        out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app','return_state':True})
        assert out['ok'] and len(c.clicks)==1 and c.clicks[0]['element_token']=='s00000001:4'
    asyncio.run(run())

def test_ambiguous_or_unverified_target_never_dispatches():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    async def run():
        for elements in ([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]*2, [{'role':'AXButton','label':'Apply','element_token':'s00000001:4'}]):
            c=Cua(elements);h=HybridClient(Munim(raw),c);await h.call('get_app_state',{'app':'bundle:com.example.app'})
            out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app','return_state':True})
            assert not out['ok'] and not c.clicks
    asyncio.run(run())

def test_secure_projected_node_and_stale_snapshot_never_dispatch():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    async def run():
        for elements in (
            [{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}, {'role':'AXTextField','subrole':'AXSecureTextField'}],
            [{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000002:4'}],
        ):
            c=Cua(elements);h=HybridClient(Munim(raw),c);await h.call('get_app_state',{'app':'bundle:com.example.app'})
            out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app','return_state':True})
            assert not out['ok'] and not c.clicks
    asyncio.run(run())

def test_missing_or_truncated_munim_preflight_never_dispatches():
    async def run():
        for raw in ('App [com.example.app] pid=123 frontmost=false windows=0', 'App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"\n… truncated'):
            c=Cua([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]);h=HybridClient(Munim(raw),c)
            await h.call('get_app_state',{'app':'bundle:com.example.app'})
            out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app','return_state':True})
            assert not out['ok'] and not c.clicks
    asyncio.run(run())

def test_offscreen_nodes_are_privately_preflighted_before_click():
    base='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class OffscreenMunim:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append(args)
            return {'ok':True,'content':base if args.get('offscreen') else base+'\n… 1 off-screen elements skipped; scroll, or pass offscreen=true to list them'}
    async def run():
        m=OffscreenMunim();c=Cua([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]);h=HybridClient(m,c)
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app','return_state':True})
        assert out['ok'] and len(c.clicks)==1 and m.calls[-1]['offscreen'] is True
    asyncio.run(run())

def test_pointer_moves_to_verified_button_then_clicks():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class PointerCua(Cua):
        def __init__(self):
            super().__init__([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4','frame':{'x':100,'y':200,'w':40,'h':20}}]); self.moves=[]; self.motion=[]
        async def call_tool(self,name,args):
            if name=='set_agent_cursor_motion': self.motion.append(args);return SimpleNamespace(isError=False,structuredContent={'ok':True})
            if name=='move_cursor': self.moves.append(args);return SimpleNamespace(isError=False,structuredContent={'ok':True})
            return await super().call_tool(name,args)
    async def run():
        c=PointerCua();h=HybridClient(Munim(raw),c);await h.call('get_app_state',{'app':'bundle:com.example.app'})
        out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app','return_state':True})
        assert out['ok'] and c.moves==[{'session':h.cursor_session,'x':120,'y':210}]*2
        assert c.motion[0]['idle_hide_ms']==2000 and len(c.clicks)==1
    asyncio.run(run())

def test_unverified_pointer_frame_refuses_input():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    async def run():
        c=Cua([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4','frame':None}]);h=HybridClient(Munim(raw),c)
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app','return_state':True})
        assert not out['ok'] and not c.clicks
    asyncio.run(run())

def test_cursor_failure_does_not_dispatch_click():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class FailedCursor(Cua):
        async def call_tool(self,name,args):
            if name=='move_cursor': return SimpleNamespace(isError=True,structuredContent={'refusal':{'code':'unavailable'}})
            return await super().call_tool(name,args)
    async def run():
        c=FailedCursor([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]);h=HybridClient(Munim(raw),c)
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app','return_state':True})
        assert not out['ok'] and not c.clicks
    asyncio.run(run())

def test_plain_text_cursor_move_acknowledgement_allows_click():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class RealMoveCua(Cua):
        async def call_tool(self,name,args):
            if name=='move_cursor':
                return SimpleNamespace(isError=False,structuredContent=None,content=[SimpleNamespace(type='text',text=f"Agent cursor '{args['session']}' moved to (120.0, 210.0).")])
            return await super().call_tool(name,args)
    async def run():
        c=RealMoveCua([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]); h=HybridClient(Munim(raw),c)
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        result=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app'})
        assert result['ok'] and len(c.clicks)==1
    asyncio.run(run())


def test_unexpected_plain_text_cursor_response_never_clicks():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class WrongAck(Cua):
        async def call_tool(self,name,args):
            if name=='move_cursor': return SimpleNamespace(isError=False,structuredContent=None,content=[SimpleNamespace(type='text',text='Unexpected response')])
            return await super().call_tool(name,args)
    async def run():
        c=WrongAck([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]); h=HybridClient(Munim(raw),c)
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        result=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app'})
        assert not result['ok'] and not c.clicks
    asyncio.run(run())

def test_chrome_window_matches_unique_accessibility_title_amid_auxiliary_windows():
    raw='Google Chrome [com.google.Chrome] pid=123 frontmost=false windows=1\n── window 0: "Computer Use Practice - Google Chrome – Jack"\n  [e1] Button "Apply"'
    class ChromeWindows(Cua):
        async def call_tool(self,name,args):
            if name=='list_windows': return SimpleNamespace(isError=False,structuredContent={'windows':[{'pid':123,'window_id':7,'layer':0,'title':'Window'}, {'pid':123,'window_id':8,'layer':0,'title':''}, {'pid':123,'window_id':4,'layer':0,'title':'Computer Use Practice'}]})
            return await super().call_tool(name,args)
    async def run():
        c=ChromeWindows([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]);h=HybridClient(Munim(raw),c)
        await h.call('get_app_state',{'app':'bundle:com.google.Chrome'})
        result=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.google.Chrome'})
        assert result['ok'] and len(c.clicks)==1
    asyncio.run(run())


def test_ambiguous_chrome_window_title_is_denied():
    raw='Google Chrome [com.google.Chrome] pid=123 frontmost=false windows=1\n── window 0: "Computer Use Practice - Google Chrome – Jack"\n  [e1] Button "Apply"'
    class DuplicateWindows(Cua):
        async def call_tool(self,name,args):
            if name=='list_windows': return SimpleNamespace(isError=False,structuredContent={'windows':[{'pid':123,'window_id':4,'title':'Computer Use Practice'}, {'pid':123,'window_id':5,'title':'Computer Use Practice'}]})
            return await super().call_tool(name,args)
    async def run():
        c=DuplicateWindows([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]);h=HybridClient(Munim(raw),c)
        await h.call('get_app_state',{'app':'bundle:com.google.Chrome'})
        result=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.google.Chrome'})
        assert not result['ok'] and not c.clicks
    asyncio.run(run())

def test_clients_use_distinct_cua_sessions():
    first=HybridClient(Munim(''), Cua([]))
    second=HybridClient(Munim(''), Cua([]))
    assert first.session != second.session
    assert first.session.startswith('handsmcp-')

def test_visible_cursor_uses_separate_unpinned_session():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class RecordingCua(Cua):
        def __init__(self):
            super().__init__([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]); self.calls=[]
        async def call_tool(self,name,args):
            self.calls.append((name,dict(args)))
            return await super().call_tool(name,args)
    async def run():
        c=RecordingCua();h=HybridClient(Munim(raw),c);await h.call('get_app_state',{'app':'bundle:com.example.app'})
        out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app'})
        assert out['ok']
        move=next(a for n,a in c.calls if n=='move_cursor')
        action=next(a for n,a in c.calls if n=='click')
        assert move['session'] != action['session']
        assert any(n=='set_agent_cursor_enabled' and a=={'session':action['session'],'enabled':False} for n,a in c.calls)
        assert any(n=='set_agent_cursor_enabled' and a=={'session':move['session'],'enabled':True} for n,a in c.calls)
    asyncio.run(run())

def test_cursor_is_raised_after_cua_pins_click():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class RecordingCua(Cua):
        def __init__(self): super().__init__([{'role':'AXButton','label':'Apply','enabled':True,'element_token':'s00000001:4'}]); self.calls=[]
        async def call_tool(self,name,args): self.calls.append(name); return await super().call_tool(name,args)
    async def run():
        c=RecordingCua();h=HybridClient(Munim(raw),c);await h.call('get_app_state',{'app':'bundle:com.example.app'})
        out=await h.call('click',{'element_id':'e1','handsmcp_expected_name':'Apply','handsmcp_expected_bundle':'com.example.app'})
        assert out['ok'] and c.calls[-2:] == ['click','move_cursor']
    asyncio.run(run())

def test_confirmed_coordinate_click_reaches_restricted_native_backend():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class RecordingMunim(Munim):
        def __init__(self, text): super().__init__(text); self.calls=[]
        async def call(self, name, args):
            self.calls.append((name, dict(args)))
            if name == 'click': return {'ok':True,'content':'input dispatched'}
            return await super().call(name,args)
    async def run():
        m=RecordingMunim(raw); h=HybridClient(m,Cua([]))
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        result=await h.call('click',{'app':'bundle:com.example.app','x':120,'y':210})
        assert result['ok']
        assert m.calls[-1] == ('click',{'app':'bundle:com.example.app','x':120,'y':210})
    asyncio.run(run())

def test_coordinate_click_rejects_missing_or_mismatched_snapshot():
    class RecordingMunim(Munim):
        def __init__(self, text): super().__init__(text); self.clicks=[]
        async def call(self,name,args):
            if name=='click': self.clicks.append(args)
            return await super().call(name,args)
    async def run():
        m=RecordingMunim('App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"')
        h=HybridClient(m,Cua([]))
        assert not (await h.call('click',{'app':'bundle:com.example.app','x':1,'y':2}))['ok']
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        assert not (await h.call('click',{'app':'bundle:com.other.app','x':1,'y':2}))['ok']
        assert not m.clicks
    asyncio.run(run())

def test_coordinate_click_moves_visible_cursor_to_requested_point():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class PointerCua(Cua):
        def __init__(self): super().__init__([]); self.moves=[]
        async def call_tool(self,name,args):
            if name=='move_cursor': self.moves.append(dict(args))
            return await super().call_tool(name,args)
    async def run():
        c=PointerCua(); h=HybridClient(Munim(raw),c)
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        assert (await h.call('click',{'app':'bundle:com.example.app','x':120,'y':210}))['ok']
        assert c.moves and c.moves[0] == {'session':h.cursor_session,'x':120,'y':210}
    asyncio.run(run())

def test_coordinate_click_does_not_dispatch_when_cursor_setup_fails():
    raw='App [com.example.app] pid=123 frontmost=false windows=1\n── window 0: "Example"\n  [e1] Button "Apply"'
    class RecordingMunim(Munim):
        def __init__(self,text): super().__init__(text); self.clicks=[]
        async def call(self,name,args):
            if name=='click': self.clicks.append(args)
            return await super().call(name,args)
    class BrokenCursor(Cua):
        async def call_tool(self,name,args):
            if name=='move_cursor': return SimpleNamespace(isError=True,structuredContent={'refusal':{}})
            return await super().call_tool(name,args)
    async def run():
        m=RecordingMunim(raw); h=HybridClient(m,BrokenCursor([]))
        await h.call('get_app_state',{'app':'bundle:com.example.app'})
        result=await h.call('click',{'app':'bundle:com.example.app','x':120,'y':210})
        assert result.get('code')=='cursor_unavailable' and not m.clicks
    asyncio.run(run())
