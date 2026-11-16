import pytest
from handsmcp.gateway import Gateway

class FakeBackend:
    def __init__(self): self.calls = []
    async def call(self, name, args):
        self.calls.append((name, args))
        return {'ok': True, 'content': '[{"bundle_id":"com.apple.TextEdit","pid":1}]'}

@pytest.mark.anyio
async def test_list_applications_uses_munim_inventory():
    backend = FakeBackend()
    result = await Gateway(backend).dispatch('desktop', 'list_applications', {})
    assert backend.calls == [('list_apps', {})]
    assert result['content'] == 'com.apple.TextEdit pid=1'

@pytest.mark.anyio
async def test_mutations_do_not_reach_backend_before_policy_is_integrated():
    backend = FakeBackend()
    result = await Gateway(backend).dispatch('computer', 'click', {'ref': 'r1'})
    assert not result['ok'] and result['code'] == 'not_implemented'
    assert backend.calls == []

@pytest.mark.anyio
async def test_concurrent_requests_are_serialized_within_process():
    import asyncio
    class Backend:
        def __init__(self): self.active = self.peak = 0
        async def call(self, name, args):
            self.active += 1
            self.peak = max(self.peak, self.active)
            await asyncio.sleep(0.01)
            self.active -= 1
            return {'ok': True}
    backend = Backend()
    gateway = Gateway(backend)
    await asyncio.gather(*(gateway.dispatch('desktop', 'list_applications', {}) for _ in range(3)))
    assert backend.peak == 1

@pytest.fixture
def anyio_backend(): return 'asyncio'

@pytest.mark.anyio
async def test_audit_failure_prevents_backend_call():
    class BrokenAudit:
        def record(self, **kwargs): raise OSError('not writable')
    backend = FakeBackend()
    result = await Gateway(backend, audit=BrokenAudit()).dispatch('desktop', 'list_applications', {})
    assert result['ok'] is False and result['code'] == 'audit_unavailable'
    assert not backend.calls

@pytest.mark.anyio
async def test_panic_stop_prevents_even_read_calls(tmp_path):
    stop = tmp_path / 'STOP'
    stop.touch()
    backend = FakeBackend()
    result = await Gateway(backend, stop_file=stop).dispatch('desktop', 'list_applications', {})
    assert result['ok'] is False and result['code'] == 'panic_stop'
    assert not backend.calls

@pytest.mark.anyio
async def test_redaction_reports_a_safe_category_without_leaking_contents():
    class Backend:
        def __init__(self): self.content='App [com.example.editor] pid=1\n  [e2] SecureTextField "SECRET_VALUE"'
        async def call(self,name,args): return {'ok':True,'content':self.content}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    result=await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    assert result['content']=='[redacted: secure field detected]'
    assert result['redaction_reason']=='secure_field_marker'
    assert 'SECRET_VALUE' not in repr(result)
    backend.content='App [com.example.editor] pid=1\n  [e2] UnverifiedTextField "SECRET_VALUE"'
    result=await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    assert result['redaction_reason']=='unverified_field_content'
    assert 'SECRET_VALUE' not in repr(result)

def test_unverified_field_diagnostic_contains_only_fixed_booleans():
    from handsmcp.gateway import unverified_field_diagnostic
    text='App\n  [e2] UnverifiedTextField "SECRET_VALUE"\n    [e3] StaticText "SECRET_CHILD"'
    result=unverified_field_diagnostic(text)
    assert result == {'nonempty_field_line':True, 'has_descendants':True}
    assert 'SECRET' not in repr(result)

def test_unverified_field_subtree_is_removed_without_losing_safe_siblings():
    from handsmcp.gateway import redact_secure_fields
    text=('App\n  [e1] Window "Example"\n    [e2] UnverifiedTextField\n'
          '      [e3] StaticText "SECRET_CHILD"\n    [e4] Button "Safe Button"')
    result=redact_secure_fields(text)
    assert 'SECRET' not in result
    assert '[e2] UnverifiedTextField' in result
    assert '[e4] Button "Safe Button"' in result

def test_folded_unverified_fields_still_suppress_entire_response():
    from handsmcp.gateway import redact_secure_fields
    text='App\n  [e1] Row [e2] UnverifiedTextField "SECRET_VALUE"'
    assert redact_secure_fields(text)=='[redacted: secure field detected]'

@pytest.mark.anyio
async def test_inspection_redacts_secure_field_line():
    class Backend:
        async def call(self, name, args):
            assert name == 'get_app_state'
            assert args['app'] == 'bundle:com.apple.TextEdit' and args['ocr'] == 'never'
            return {'ok': True, 'content': 'TextEdit [com.apple.TextEdit] pid=1\n  [e1] SecureTextField "secret" value="secret" (focused)\n  [e2] Button "Save"'}
    result = await Gateway(Backend()).dispatch('computer', 'inspect', {'app': 'TextEdit'})
    assert result['ok'] is True
    assert 'secret' not in result['content']
    assert result['content'] == '[redacted: secure field detected]'

@pytest.mark.anyio
async def test_inspect_denies_unreviewed_app_without_calling_backend():
    backend = FakeBackend()
    result = await Gateway(backend).dispatch('computer', 'inspect', {'app':'1Password'})
    assert result['ok'] is False and not backend.calls

@pytest.mark.anyio
async def test_calculator_click_requires_confirmation_and_is_single_use():
    class Backend:
        def __init__(self): self.calls = []
        async def call(self, name, args):
            self.calls.append((name,args))
            if name == 'get_app_state':
                return {'ok':True,'content':'Calculator [com.apple.calculator] pid=1\n  [e1] Button "2"'}
            return {'ok':True,'content':'pressed 2'}
    backend = Backend(); gateway = Gateway(backend)
    await gateway.dispatch('computer','inspect',{'app':'Calculator'})
    request = await gateway.dispatch('computer','click',{'ref':'e1'})
    assert request['code'] == 'confirmation_required' and len(backend.calls) == 1
    token = request['token']
    result = await gateway.dispatch('computer','confirm',{'token':token})
    assert result['ok'] and backend.calls[-1][0] == 'click'
    assert (await gateway.dispatch('computer','confirm',{'token':token}))['ok'] is False
    assert len(backend.calls) == 3

@pytest.mark.anyio
async def test_reinspection_invalidates_pending_confirmation():
    class Backend:
        def __init__(self): self.calls = []
        async def call(self, name, args):
            self.calls.append(name)
            return {'ok': True, 'content':'Calculator [com.apple.calculator] pid=1\n  [e1] Button "2"'}
    backend = Backend(); gateway = Gateway(backend)
    await gateway.dispatch('computer','inspect',{'app':'Calculator'})
    pending = await gateway.dispatch('computer','click',{'ref':'e1'})
    await gateway.dispatch('computer','inspect',{'app':'Calculator'})
    result = await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['ok'] is False and result['code'] == 'stale_ref'
    assert backend.calls == ['get_app_state','get_app_state']

def test_secure_field_is_redacted_even_when_folded_into_a_row():
    from handsmcp.gateway import redact_secure_fields
    result = redact_secure_fields('  [e1] Row, [e2] SecureTextField "secret" value="secret"')
    assert 'secret' not in result.lower()
    assert 'redacted' in result

@pytest.mark.anyio
async def test_other_server_action_invalidates_pending_confirmation(tmp_path):
    from handsmcp.desktop_lock import DesktopLock
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append(name)
            return {'ok':True,'content':'Calculator [com.apple.calculator] pid=1\n  [e1] Button "2"'}
    path = tmp_path / 'desktop.lock'
    backend1, backend2 = Backend(), Backend()
    one = Gateway(backend1, desktop_lock=DesktopLock(path))
    two = Gateway(backend2, desktop_lock=DesktopLock(path))
    await one.dispatch('computer','inspect',{'app':'Calculator'})
    first = await one.dispatch('computer','click',{'ref':'e1'})
    await two.dispatch('computer','inspect',{'app':'Calculator'})
    second = await two.dispatch('computer','click',{'ref':'e1'})
    assert (await two.dispatch('computer','confirm',{'token':second['token']}))['ok']
    result = await one.dispatch('computer','confirm',{'token':first['token']})
    assert result['code'] == 'stale_ref' and backend1.calls == ['get_app_state']

@pytest.mark.anyio
async def test_input_transport_error_reports_uncertain_emission():
    class Backend:
        async def call(self, name, args):
            if name == 'click': raise OSError('connection lost')
            return {'ok':True, 'content':'Calculator [com.apple.calculator] pid=1\n  [e1] Button "2"'}
    gateway = Gateway(Backend())
    await gateway.dispatch('computer','inspect',{'app':'Calculator'})
    pending = await gateway.dispatch('computer','click',{'ref':'e1'})
    result = await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['ok'] is False
    assert result['code'] == 'upstream_error'
    assert result['input_emitted'] is None and result['completed'] is False
    assert 'Inspect' in result['hint']

@pytest.mark.anyio
async def test_launch_application_uses_validated_bundle_and_respects_modes():
    launched=[]
    async def launcher(bundle):
        launched.append(bundle)
        return {'ok':True,'message':'launch requested'}
    gateway=Gateway(FakeBackend(),allowed_apps=['*'],app_launcher=launcher)
    result=await gateway.dispatch('desktop','launch_application',{'app':'com.apple.calculator'})
    assert result['ok'] and result['completed'] is False and launched==['com.apple.calculator']
    assert not (await gateway.dispatch('desktop','launch_application',{'app':'com.1password.1password'}))['ok']
    assert not (await gateway.dispatch('desktop','launch_application',{'app':'com.example.app;rm'}))['ok']
    readonly=Gateway(FakeBackend(),mode='read_only',allowed_apps=['*'],app_launcher=launcher)
    assert (await readonly.dispatch('desktop','launch_application',{'app':'com.apple.calculator'}))['code']=='read_only'
    assert launched==['com.apple.calculator']

@pytest.mark.anyio
async def test_launch_failure_is_not_reported_as_success():
    async def launcher(bundle): return {'ok':False,'code':'launch_failed'}
    gateway=Gateway(FakeBackend(),allowed_apps=['*'],app_launcher=launcher)
    result=await gateway.dispatch('desktop','launch_application',{'app':'com.example.missing'})
    assert not result['ok'] and result['code']=='launch_failed'

@pytest.mark.anyio
async def test_activate_app_is_targeted_invalidates_snapshots_and_respects_policy():
    class Lease:
        def __init__(self): self.value=0
        def generation(self): return self.value
        def advance(self): self.value+=1
    class Lock:
        def __init__(self): self.lease=Lease()
        def hold(self):
            from contextlib import asynccontextmanager
            @asynccontextmanager
            async def hold(): yield self.lease
            return hold()
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name == 'get_app_state': return {'ok':True, 'content':'TextEdit [com.apple.TextEdit] pid=1\n  [e3] TextArea (focused)'}
            return {'ok':True,'content':'activated app'}
    backend=Backend(); gateway=Gateway(backend, desktop_lock=Lock(), allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'TextEdit'})
    pending=await gateway.dispatch('computer','type',{'ref':'e3','text':'hello'})
    result=await gateway.dispatch('desktop','activate_application',{'app':'net.whatsapp.WhatsApp'})
    assert result['ok'] and backend.calls[-1]==('activate_app',{'app':'bundle:net.whatsapp.WhatsApp'})
    assert (await gateway.dispatch('computer','confirm',{'token':pending['token']}))['code']=='stale_ref'
    assert not (await gateway.dispatch('desktop','activate_application',{'app':'com.1password.1password'}))['ok']
    readonly=Gateway(backend,mode='read_only',allowed_apps=['*'])
    assert not (await readonly.dispatch('desktop','activate_application',{'app':'net.whatsapp.WhatsApp'}))['ok']

@pytest.mark.anyio
async def test_screenshot_checks_complete_single_window_and_denies_secure_fields():
    class Backend:
        def __init__(self): self.calls=[]; self.tree='App [com.example.editor] pid=1 frontmost=false windows=1\n── window 0: "Document"\n  [e1] Window "Document"'
        async def call(self,name,args): self.calls.append((name,args)); return {'ok':True,'content':self.tree}
        async def capture(self,args): self.calls.append(('capture',args)); return {'ok':True,'image':'image fixture','message':'captured'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    result=await gateway.capture_image('com.example.editor')
    assert result['ok'] and result['image']=='image fixture'
    assert backend.calls[-1]==('capture',{'app':'bundle:com.example.editor'})
    for unsafe in ['  [e2] SecureTextField', '  [e2] UnverifiedTextField', '... 2 off-screen elements skipped']:
        backend.tree='App [com.example.editor] pid=1 frontmost=false windows=1\n── window 0: "Document"\n  [e1] Window "Document"\n'+unsafe
        count=len(backend.calls)
        result=await gateway.capture_image('com.example.editor')
        assert not result['ok'] and len(backend.calls)==count+1 and backend.calls[-1][0]=='get_app_state'
    assert not (await gateway.capture_image('com.1password.1password'))['ok']

@pytest.mark.anyio
async def test_screenshot_allows_multiple_fully_checked_windows_but_rejects_no_window():
    class Backend:
        def __init__(self): self.count=2
        async def call(self,name,args): return {'ok':True,'content':f'App [com.example.editor] pid=1 frontmost=false windows={self.count}\n── window 0: "One"\n  [e1] Window "One"\n── window 1: "Two"\n  [e2] Window "Two"'}
        async def capture(self,args): return {'ok':True,'image':'fixture','message':'largest window'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    assert (await gateway.capture_image('com.example.editor'))['ok']
    backend.count=3
    assert not (await gateway.capture_image('com.example.editor'))['ok']
    backend.count=0
    result=await gateway.capture_image('com.example.editor')
    assert result['ok'] is False

@pytest.mark.anyio
async def test_screenshot_invalidates_pending_ref_and_does_not_capture_after_stop(tmp_path):
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append(name)
            return {'ok':True,'content':'App [com.example.editor] pid=1 frontmost=false windows=1\n── window 0: "Document"\n  [e1] Window "Document"\n    [e2] Button "OK"'}
        async def capture(self,args): self.calls.append('capture'); return {'ok':True,'image':'fixture'}
    stop=tmp_path/'STOP'; backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'],stop_file=stop)
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    pending=await gateway.dispatch('computer','click',{'ref':'e2'})
    assert (await gateway.capture_image('com.example.editor'))['ok']
    assert (await gateway.dispatch('computer','confirm',{'token':pending['token']}))['code']=='stale_ref'
    stop.touch(); calls=len(backend.calls)
    assert (await gateway.capture_image('com.example.editor'))['code']=='panic_stop'
    assert len(backend.calls)==calls

@pytest.mark.anyio
async def test_general_scroll_requires_a_confirmed_unchanged_app():
    class Backend:
        def __init__(self): self.calls=[]; self.pid=77
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state': return {'ok':True,'content':f'App [com.example.editor] pid={self.pid} frontmost=true windows=1\n  [e1] Window "Document"'}
            return {'ok':True,'content':'scrolled down'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    assert (await gateway.dispatch('computer','scroll',{'direction':'down','amount':4}))['code']=='not_implemented'
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    pending=await gateway.dispatch('computer','scroll',{'direction':'down','amount':4})
    assert pending['code']=='confirmation_required' and len(backend.calls)==1
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['ok'] and backend.calls[-1]==('scroll',{'app':'bundle:com.example.editor','direction':'down','amount':4})
    assert not (await gateway.dispatch('computer','confirm',{'token':pending['token']}))['ok']
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    pending=await gateway.dispatch('computer','scroll',{'direction':'up','amount':1})
    backend.pid=78
    assert (await gateway.dispatch('computer','confirm',{'token':pending['token']}))['code']=='stale_ref'
    assert (await gateway.dispatch('computer','scroll',{'direction':'down','amount':101}))['code']=='not_implemented'
    readonly=Gateway(backend,mode='read_only',allowed_apps=['*'])
    await readonly.dispatch('computer','inspect',{'app':'com.example.editor'})
    assert (await readonly.dispatch('computer','scroll',{'direction':'down','amount':1}))['code']=='read_only'

@pytest.mark.anyio
async def test_unavailable_menu_shortcut_is_reported_without_claiming_input():
    class Backend:
        async def call(self,name,args):
            if name == 'get_app_state': return {'ok':True,'content':'App [com.example.editor] pid=1 frontmost=false windows=1\n  [e1] Window "Document"'}
            return {'ok':False,'content':'error: File > Save is disabled or cannot be verified; no menu action was sent'}
    gateway=Gateway(Backend(),allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    pending=await gateway.dispatch('computer','press_key',{'key':'s','modifiers':['cmd']})
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['code']=='shortcut_unavailable' and result['input_emitted'] is False and result['completed'] is False

@pytest.mark.anyio
async def test_general_navigation_shortcuts_are_confirmed_for_a_verified_app():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name == 'get_app_state':
                return {'ok':True,'content':'App [com.example.editor] pid=77 frontmost=true windows=1\n  [e1] Window "Document"'}
            return {'ok':True,'content':'pressed cmd+s'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    pending=await gateway.dispatch('computer','press_key',{'key':'s','modifiers':['cmd']})
    assert pending['code']=='confirmation_required' and len(backend.calls)==1
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['ok'] and backend.calls[-1]==('press_key',{'app':'bundle:com.example.editor','key':'s','modifiers':['cmd']})
    assert not (await gateway.dispatch('computer','confirm',{'token':pending['token']}))['ok']

@pytest.mark.anyio
async def test_general_keys_block_text_input_unverified_state_and_read_only():
    class Backend:
        async def call(self,name,args): return {'ok':True,'content':'App [com.example.editor] pid=77 frontmost=true windows=1\n  [e1] Window "Document"'}
    gateway=Gateway(Backend(),allowed_apps=['*'])
    assert not (await gateway.dispatch('computer','press_key',{'key':'return','modifiers':[]}))['ok']
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    assert (await gateway.dispatch('computer','press_key',{'key':'x','modifiers':[]}))['code']=='not_implemented'
    assert (await gateway.dispatch('computer','press_key',{'key':'x','modifiers':['shift']}))['code']=='not_implemented'
    assert (await gateway.dispatch('computer','press_key',{'key':'v','modifiers':['cmd']}))['code']=='not_implemented'
    assert (await gateway.dispatch('computer','press_key',{'key':'return','modifiers':[]}))['code']=='confirmation_required'
    read_only=Gateway(Backend(),mode='read_only',allowed_apps=['*'])
    await read_only.dispatch('computer','inspect',{'app':'com.example.editor'})
    assert (await read_only.dispatch('computer','press_key',{'key':'return','modifiers':[]}))['code']=='read_only'

@pytest.mark.anyio
async def test_textedit_new_document_shortcut_requires_confirmation_and_rechecks_snapshot():
    class Backend:
        def __init__(self): self.calls = []
        async def call(self, name, args):
            self.calls.append((name, args))
            if name == 'get_app_state':
                return {'ok': True, 'content': 'TextEdit [com.apple.TextEdit] pid=1 frontmost=false windows=0'}
            return {'ok': True, 'content': 'pressed command+n'}
    backend = Backend()
    gateway = Gateway(backend)
    await gateway.dispatch('computer', 'inspect', {'app':'TextEdit'})
    pending = await gateway.dispatch('computer', 'press_key', {'key':'n', 'modifiers':['cmd']})
    assert pending['code'] == 'confirmation_required'
    assert len(backend.calls) == 1
    result = await gateway.dispatch('computer', 'confirm', {'token':pending['token']})
    assert result['ok']
    assert backend.calls[-1] == ('press_key', {'app':'bundle:com.apple.TextEdit', 'key':'n', 'modifiers':['cmd']})
    assert not (await gateway.dispatch('computer', 'confirm', {'token':pending['token']}))['ok']

@pytest.mark.anyio
async def test_shortcuts_other_than_new_document_remain_disabled():
    class Backend:
        async def call(self, name, args):
            return {'ok': True, 'content': 'TextEdit [com.apple.TextEdit] pid=1 frontmost=false windows=0'}
    gateway = Gateway(Backend())
    await gateway.dispatch('computer', 'inspect', {'app':'TextEdit'})
    assert (await gateway.dispatch('computer', 'press_key', {'key':'f21', 'modifiers':['cmd']}))['code'] == 'not_implemented'
    assert (await gateway.dispatch('computer', 'press_key', {'key':'n', 'modifiers':[]}))['code'] == 'not_implemented'

@pytest.mark.anyio
async def test_new_document_shortcut_fails_closed_on_changed_process_and_read_only():
    class Backend:
        def __init__(self): self.pid = 1; self.calls = []
        async def call(self, name, args):
            self.calls.append(name)
            return {'ok': True, 'content': f'TextEdit [com.apple.TextEdit] pid={self.pid} frontmost=false windows=0'}
    backend = Backend(); gateway = Gateway(backend)
    await gateway.dispatch('computer', 'inspect', {'app':'TextEdit'})
    request = await gateway.dispatch('computer', 'press_key', {'key':'n', 'modifiers':['cmd']})
    backend.pid = 2
    assert (await gateway.dispatch('computer', 'confirm', {'token':request['token']}))['code'] == 'stale_ref'
    assert 'press_key' not in backend.calls
    read_only = Gateway(backend, mode='read_only')
    await read_only.dispatch('computer', 'inspect', {'app':'TextEdit'})
    assert (await read_only.dispatch('computer', 'press_key', {'key':'n', 'modifiers':['cmd']}))['code'] == 'read_only'

@pytest.mark.anyio
async def test_textedit_accepts_multiline_text_but_other_apps_do_not():
    class Backend:
        async def call(self, name, args):
            bundle = args['app'].removeprefix('bundle:')
            return {'ok': True, 'content': f'App [{bundle}] pid=1\n  [e3] TextArea (focused)'}
    gateway = Gateway(Backend(), allowed_apps=['*'])
    await gateway.dispatch('computer', 'inspect', {'app': 'com.apple.TextEdit'})
    result = await gateway.dispatch('computer', 'type', {'ref':'e3', 'text':'one\ntwo\tthree'})
    assert result['code'] == 'confirmation_required'
    await gateway.dispatch('computer', 'inspect', {'app': 'net.whatsapp.WhatsApp'})
    result = await gateway.dispatch('computer', 'type', {'ref':'e3', 'text':'one\ntwo'})
    assert result['code'] == 'not_implemented'

@pytest.mark.anyio
async def test_long_field_replacement_is_confirmed_and_bounded():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state': return {'ok':True,'content':'TextEdit [com.apple.TextEdit] pid=1\n  [e3] TextArea (focused)'}
            return {'ok':True,'content':'replaced'}
    backend=Backend(); gateway=Gateway(backend)
    await gateway.dispatch('computer','inspect',{'app':'TextEdit'})
    value='x'*500
    assert (await gateway.dispatch('computer','type',{'ref':'e3','text':value}))['code']=='not_implemented'
    pending=await gateway.dispatch('computer','type',{'ref':'e3','text':value,'mode':'replace'})
    assert pending['code']=='confirmation_required' and len(backend.calls)==1
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['ok'] and backend.calls[-1][0]=='set_value' and backend.calls[-1][1]['value']==value
    await gateway.dispatch('computer','inspect',{'app':'TextEdit'})
    assert (await gateway.dispatch('computer','type',{'ref':'e3','text':'x'*2001,'mode':'replace'}))['code']=='not_implemented'

@pytest.mark.anyio
async def test_verified_text_fields_are_usable_but_unverified_fields_remain_blocked():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state':
                return {'ok':True,'content':'App [com.example.editor] pid=1 frontmost=true windows=1\n  [e2] TextField "Search"\n  [e3] UnverifiedTextField'}
            return {'ok':True,'content':'typed'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    assert (await gateway.dispatch('computer','type',{'ref':'e3','text':'test'}))['code']=='not_implemented'
    request=await gateway.dispatch('computer','type',{'ref':'e2','text':'hello'})
    assert request['code']=='confirmation_required' and len(backend.calls)==1
    result=await gateway.dispatch('computer','confirm',{'token':request['token']})
    assert result['ok']
    assert backend.calls[-1] == ('type_text',{'element_id':'e2','text':'hello','return_state':True,'handsmcp_expected_bundle':'com.example.editor','handsmcp_expected_role':'AXTextField'})

@pytest.mark.anyio
async def test_textedit_typing_requires_confirmation_and_never_logs_text():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name == 'get_app_state': return {'ok':True,'content':'TextEdit [com.apple.TextEdit] pid=1\n  [e3] TextArea (focused)'}
            return {'ok':True,'content':'typed'}
    class Audit:
        def __init__(self): self.records=[]
        def record(self, **kwargs): self.records.append(kwargs)
    backend=Backend(); audit=Audit(); gateway=Gateway(backend,audit=audit)
    await gateway.dispatch('computer','inspect',{'app':'TextEdit'})
    pending=await gateway.dispatch('computer','type',{'ref':'e3','text':'hello test'})
    assert pending['code']=='confirmation_required' and len(backend.calls)==1
    assert 'hello test' in pending['summary']
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['ok'] and backend.calls[-1] == ('type_text',{'element_id':'e3','text':'hello test','return_state':True,'handsmcp_expected_bundle':'com.apple.TextEdit'})
    assert 'hello test' not in repr(audit.records)

@pytest.mark.anyio
async def test_typing_is_rejected_for_secure_or_unseen_ref():
    backend=FakeBackend(); gateway=Gateway(backend)
    result=await gateway.dispatch('computer','type',{'ref':'e999','text':'secret'})
    assert result['ok'] is False and not backend.calls

@pytest.mark.anyio
async def test_rate_limit_blocks_requests_before_backend():
    from handsmcp.rate_limit import RateLimiter
    backend = FakeBackend(); gateway = Gateway(backend)
    gateway.rate_limiter = RateLimiter(limit=1, window_seconds=60, clock=lambda: 0)
    assert (await gateway.dispatch('desktop','list_applications',{}))['ok']
    result = await gateway.dispatch('desktop','list_applications',{})
    assert result['code'] == 'rate_limited' and len(backend.calls) == 1

@pytest.mark.anyio
@pytest.mark.parametrize('mode', ['read_only','dry_run'])
async def test_non_normal_modes_block_inputs_even_if_snapshot_is_valid(mode):
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append(name)
            return {'ok':True,'content':'Calculator [com.apple.calculator] pid=1\n  [e1] Button "2"'}
    backend=Backend(); gateway=Gateway(backend,mode=mode)
    await gateway.dispatch('computer','inspect',{'app':'Calculator'})
    response=await gateway.dispatch('computer','click',{'ref':'e1'})
    assert response['ok'] is False and response['code']==mode and backend.calls==['get_app_state']

@pytest.mark.anyio
async def test_action_result_redacts_secure_field_lines():
    class Backend:
        async def call(self,name,args):
            if name=='get_app_state': return {'ok':True,'content':'Calculator [com.apple.calculator] pid=1\n  [e1] Button "2"'}
            return {'ok':True,'content':'pressed\n  [e4] SecureTextField "password123"'}
    gateway=Gateway(Backend())
    await gateway.dispatch('computer','inspect',{'app':'Calculator'})
    pending=await gateway.dispatch('computer','click',{'ref':'e1'})
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert 'password123' not in result['content'] and 'redacted' in result['content']

@pytest.mark.anyio
async def test_external_change_before_confirmation_blocks_click():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append(name)
            if name == 'get_app_state':
                value='2' if self.calls.count('get_app_state') == 1 else '8'
                return {'ok':True,'content':f'Calculator [com.apple.calculator] pid=1\n  [e1] Button "{value}"'}
            return {'ok':True,'content':'pressed'}
    backend=Backend(); gateway=Gateway(backend)
    await gateway.dispatch('computer','inspect',{'app':'Calculator'})
    pending=await gateway.dispatch('computer','click',{'ref':'e1'})
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['code']=='stale_ref' and backend.calls==['get_app_state','get_app_state']

@pytest.mark.anyio
async def test_multiline_secure_field_never_returns_subtree():
    class Backend:
        async def call(self,name,args):
            return {'ok':True,'content':'TextEdit [com.apple.TextEdit] pid=1\n  [e1] SecureTextField\n    value="sensitive-pass"\n  [e2] Button "OK"'}
    result=await Gateway(Backend()).dispatch('computer','inspect',{'app':'TextEdit'})
    assert 'sensitive-pass' not in result['content']

@pytest.mark.anyio
async def test_window_listing_is_not_mislabeled_application_inventory():
    backend=FakeBackend()
    result=await Gateway(backend).dispatch('desktop','list_windows',{})
    assert result['code']=='not_implemented' and backend.calls==[]

@pytest.mark.anyio
async def test_inventory_transport_failure_is_not_lock_failure():
    class Backend:
        async def call(self,name,args): raise OSError('child exited')
    result=await Gateway(Backend()).dispatch('desktop','list_applications',{})
    assert result['code']=='upstream_error' and result['input_emitted'] is False

@pytest.mark.anyio
async def test_response_contains_honest_execution_metadata():
    backend = FakeBackend(); gateway = Gateway(backend)
    result = await gateway.dispatch('desktop','list_applications',{})
    assert result['input_emitted'] is False and result['changed'] is None
    assert result['completed'] is True and result['policy'] == 'allow'
    assert isinstance(result['elapsed_ms'], (int,float)) and result['elapsed_ms'] >= 0

def test_masked_textfield_output_is_suppressed():
    from handsmcp.gateway import redact_secure_fields
    result=redact_secure_fields('Fixture\n  [e2] TextField "" (focused)')
    assert result == '[redacted: secure field detected]'

@pytest.mark.anyio
async def test_textedit_replace_uses_confirmed_set_value():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name == 'get_app_state': return {'ok':True,'content':'TextEdit [com.apple.TextEdit] pid=1\n  [e3] TextArea "old"'}
            return {'ok':True,'content':'value set'}
    backend=Backend(); gateway=Gateway(backend)
    await gateway.dispatch('computer','inspect',{'app':'TextEdit'})
    pending=await gateway.dispatch('computer','type',{'ref':'e3','text':'replacement','mode':'replace'})
    assert pending['code']=='confirmation_required'
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['ok'] and backend.calls[-1] == ('set_value',{'element_id':'e3','value':'replacement','return_state':True,'handsmcp_expected_bundle':'com.apple.TextEdit'})

@pytest.mark.anyio
async def test_custom_allowed_bundle_can_be_inspected_with_identity_check():
    class Backend:
        async def call(self,name,args):
            assert name=='get_app_state' and args['app']=='bundle:com.example.safe'
            return {'ok':True,'content':'Example [com.example.safe] pid=123 windows=1\n  [e1] Button "Test"'}
    gateway=Gateway(Backend(), allowed_apps=('com.example.safe',))
    result=await gateway.dispatch('computer','inspect',{'app':'com.example.safe'})
    assert result['ok'] and 'Button' in result['content']

@pytest.mark.anyio
async def test_wrong_bundle_identity_is_rejected_without_returning_content():
    class Backend:
        async def call(self,name,args): return {'ok':True,'content':'Other [com.example.other] pid=123\nsecret'}
    gateway=Gateway(Backend(), allowed_apps=('com.example.safe',))
    result=await gateway.dispatch('computer','inspect',{'app':'com.example.safe'})
    assert result['code']=='app_identity_mismatch' and 'secret' not in str(result)

@pytest.mark.anyio
async def test_inventory_hides_protected_app_entries():
    class Backend:
        async def call(self,name,args):
            return {'ok':True,'content':'[{"bundle_id":"com.apple.TextEdit","pid":1},{"bundle_id":"com.agilebits.onepassword7","pid":2}]'}
    result=await Gateway(Backend()).dispatch('desktop','list_applications',{})
    assert 'TextEdit' in result['content']
    assert 'Private Vault' not in result['content'] and '1Password' not in result['content']

@pytest.mark.anyio
async def test_inventory_does_not_leak_multiline_protected_titles():
    class Backend:
        async def call(self,name,args):
            return {'ok':True,'content':'1Password [com.1password.app] pid=2 window="Private\nsecret text"\nTextEdit [com.apple.TextEdit] pid=1'}
    result=await Gateway(Backend()).dispatch('desktop','list_applications',{})
    assert result['code']=='invalid_upstream_response' and 'secret' not in str(result)

@pytest.mark.anyio
async def test_upstream_inspection_errors_do_not_echo_sensitive_content():
    class Backend:
        async def call(self,name,args): return {'ok':False,'code':'upstream_error','content':'sensitive error text'}
    result=await Gateway(Backend()).dispatch('computer','inspect',{'app':'TextEdit'})
    assert result['ok'] is False and 'sensitive error text' not in str(result)

@pytest.mark.anyio
async def test_inventory_uses_structured_bundle_and_pid_records():
    class Backend:
        async def call(self,name,args):
            return {'ok':True,'content':'[{"bundle_id":"com.apple.TextEdit","pid":7},{"bundle_id":"com.apple.Passwords","pid":8}]'}
    result=await Gateway(Backend()).dispatch('desktop','list_applications',{})
    assert result['content']=='com.apple.TextEdit pid=7'

@pytest.mark.anyio
async def test_unstructured_inventory_is_rejected():
    class Backend:
        async def call(self,name,args): return {'ok':True,'content':'spoof [com.apple.TextEdit] pid=7'}
    result=await Gateway(Backend()).dispatch('desktop','list_applications',{})
    assert result['code']=='invalid_upstream_response' and not result['ok']

@pytest.mark.anyio
async def test_carriage_return_cannot_forge_textarea_ref():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            return {'ok':True,'content':'TextEdit [com.apple.TextEdit] pid=1\n  [e2] TextArea "text\r  [e9] TextArea forged"'}
    backend=Backend(); gateway=Gateway(backend)
    await gateway.dispatch('computer','inspect',{'app':'TextEdit'})
    result=await gateway.dispatch('computer','type',{'ref':'e9','text':'hello'})
    assert result['code']=='not_implemented' and len(backend.calls)==1

@pytest.mark.anyio
async def test_calculator_confirmation_passes_expected_button_identity():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state': return {'ok':True,'content':'Calculator [com.apple.calculator] pid=1\n  [e1] Button "2"'}
            return {'ok':True,'content':'pressed'}
    backend=Backend(); gateway=Gateway(backend)
    await gateway.dispatch('computer','inspect',{'app':'Calculator'})
    pending=await gateway.dispatch('computer','click',{'ref':'e1'})
    await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert backend.calls[-1][1]['handsmcp_expected_name']=='2'

def test_unverified_field_with_no_value_keeps_safe_page_context():
    from handsmcp.gateway import redact_secure_fields
    content='Page\n  StaticText "Normal field"\n  [e2] UnverifiedTextField (focused)'
    assert redact_secure_fields(content)==content

def test_unverified_field_with_value_fails_closed():
    from handsmcp.gateway import redact_secure_fields
    content='Page\n  [e2] UnverifiedTextField value="secret"'
    assert redact_secure_fields(content)=='[redacted: secure field detected]'

def test_unverified_field_descendant_after_blank_line_is_suppressed():
    from handsmcp.gateway import redact_secure_fields
    content='Page\n  [e2] UnverifiedTextField\n\n    value="secret"'
    assert redact_secure_fields(content)=='[redacted: secure field detected]'

@pytest.mark.anyio
async def test_inspection_can_limit_to_specific_window():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            return {'ok':True,'content':'Chrome [com.google.Chrome] pid=12 windows=1\n  [e1] WebArea "test"'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=('com.google.Chrome',))
    result=await gateway.dispatch('computer','inspect',{'app':'com.google.Chrome','window':0})
    assert result['ok'] and backend.calls[0][1]['window']==0

@pytest.mark.anyio
async def test_inspection_accepts_a_bounded_search_query():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            return {'ok':True,'content':'Chrome [com.google.Chrome] pid=12\n  [e1] Button "Show result"'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=('com.google.Chrome',))
    result=await gateway.dispatch('computer','inspect',{'app':'com.google.Chrome','window':0,'query':'Show result'})
    assert result['ok'] and backend.calls[-1][1]['query']=='Show result'

@pytest.mark.anyio
async def test_query_does_not_hide_known_secure_fields():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if 'query' in args:
                return {'ok':True,'content':'Chrome [com.google.Chrome] pid=12\n  [e1] Button "Show result"'}
            return {'ok':True,'content':'Chrome [com.google.Chrome] pid=12\n  [e2] SecureTextField'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=('com.google.Chrome',))
    result=await gateway.dispatch('computer','inspect',{'app':'com.google.Chrome','query':'Show result'})
    assert result['content']=='[redacted: secure field detected]' and len(backend.calls)==1

@pytest.mark.anyio
async def test_all_apps_inventory_and_inspection_preserve_protected_denial():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='list_apps':
                return {'ok':True,'content':'[{"bundle_id":"com.example.editor","pid":12},{"bundle_id":"com.1password.app","pid":13}]'}
            return {'ok':True,'content':'Editor [com.example.editor] pid=12\n  [e1] Button "Open"'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=('*',))
    inventory=await gateway.dispatch('desktop','list_applications',{})
    assert 'com.example.editor' in inventory['content'] and '1password' not in inventory['content']
    assert (await gateway.dispatch('computer','inspect',{'app':'com.example.editor'}))['ok']
    before=len(backend.calls)
    assert not (await gateway.dispatch('computer','inspect',{'app':'com.1password.app'}))['ok']
    assert len(backend.calls)==before

def test_normal_static_text_bullet_separator_does_not_redact():
    from handsmcp.gateway import redact_secure_fields
    content='WhatsApp\n  StaticText "Today • 10:30"'
    assert redact_secure_fields(content)==content

def test_masked_field_with_spaced_bullet_still_redacts():
    from handsmcp.gateway import redact_secure_fields
    assert redact_secure_fields('Page\n  [e1] TextField " • "')=='[redacted: secure field detected]'

@pytest.mark.anyio
async def test_generic_desktop_button_requires_confirmation_and_forwards_identity():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state': return {'ok':True,'content':'App [com.example.app] pid=1\n  [e4] Button "Open"'}
            return {'ok':True,'content':'pressed e4'}
    b=Backend(); g=Gateway(b,allowed_apps=('com.example.app',))
    await g.dispatch('computer','inspect',{'app':'com.example.app'})
    request=await g.dispatch('computer','click',{'ref':'e4'})
    assert request['code']=='confirmation_required' and len(b.calls)==1
    result=await g.dispatch('computer','confirm',{'token':request['token']})
    assert result['ok'] and b.calls[-1][1]['handsmcp_expected_bundle']=='com.example.app'

@pytest.mark.anyio
async def test_general_text_area_requires_confirmation_and_forwards_bundle():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state': return {'ok':True,'content':'App [com.example.app] pid=1\n  [e5] TextArea "Message"'}
            return {'ok':True,'content':'typed 5 characters'}
    b=Backend(); g=Gateway(b,allowed_apps=('com.example.app',))
    await g.dispatch('computer','inspect',{'app':'com.example.app'})
    request=await g.dispatch('computer','type',{'ref':'e5','text':'hello'})
    assert request['code']=='confirmation_required' and len(b.calls)==1
    result=await g.dispatch('computer','confirm',{'token':request['token']})
    assert result['ok'] and b.calls[-1][1]['handsmcp_expected_bundle']=='com.example.app'

@pytest.mark.anyio
async def test_whatsapp_new_chat_navigation_does_not_request_confirmation():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state': return {'ok':True,'content':'WhatsApp [net.whatsapp.WhatsApp] pid=1\n  [e4] Button "\u200eNew Chat"'}
            return {'ok':True,'content':'pressed e4'}
    b=Backend(); g=Gateway(b,allowed_apps=('net.whatsapp.WhatsApp',))
    await g.dispatch('computer','inspect',{'app':'net.whatsapp.WhatsApp'})
    result=await g.dispatch('computer','click',{'ref':'e4'})
    assert result['ok'] and b.calls[-1][0]=='click'

@pytest.mark.anyio
async def test_whatsapp_select_self_chat_is_navigation():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state': return {'ok':True,'content':'WhatsApp [net.whatsapp.WhatsApp] pid=1\n  [e4] Button "+91 90796 20861\u2069 (\u200eYou), \u200eMessage yourself"'}
            return {'ok':True,'content':'pressed e4'}
    b=Backend(); g=Gateway(b,allowed_apps=('net.whatsapp.WhatsApp',))
    await g.dispatch('computer','inspect',{'app':'net.whatsapp.WhatsApp'})
    result=await g.dispatch('computer','click',{'ref':'e4'})
    assert result['ok'] and b.calls[-1][0]=='click'

@pytest.mark.anyio
async def test_whatsapp_picker_contact_is_navigation_but_send_still_confirms():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name=='get_app_state': return {'ok':True,'content':'WhatsApp [net.whatsapp.WhatsApp] pid=1\n  [e1] Sheet\n    [e2] Heading "New chat"\n    [e3] Heading "Frequently contacted"\n      [e4] Group "Veer Alone"\n        [e5] Button "Veer Alone"\n    [e6] Button "Send"'}
            return {'ok':True,'content':'pressed e5'}
    b=Backend(); g=Gateway(b,allowed_apps=('net.whatsapp.WhatsApp',))
    await g.dispatch('computer','inspect',{'app':'net.whatsapp.WhatsApp'})
    assert (await g.dispatch('computer','click',{'ref':'e6'}))['code']=='confirmation_required'
    assert (await g.dispatch('computer','click',{'ref':'e5'}))['ok']
    assert b.calls[-1][0]=='click'

@pytest.mark.anyio
async def test_focus_change_alone_does_not_invalidate_confirmed_target():
    class Backend:
        def __init__(self): self.count=0; self.calls=[]
        async def call(self,name,args):
            self.calls.append(name)
            if name=='get_app_state':
                self.count+=1
                active='false' if self.count==1 else 'true'
                return {'ok':True,'content':f'WhatsApp [net.whatsapp.WhatsApp] pid=1 frontmost={active} windows=1\n  [e3] TextArea "Compose message"'}
            return {'ok':True,'content':'typed'}
    b=Backend(); g=Gateway(b,allowed_apps=('net.whatsapp.WhatsApp',))
    await g.dispatch('computer','inspect',{'app':'net.whatsapp.WhatsApp'})
    request=await g.dispatch('computer','type',{'ref':'e3','text':'hello'})
    result=await g.dispatch('computer','confirm',{'token':request['token']})
    assert result['ok'] and b.calls[-1]=='type_text'

@pytest.mark.anyio
async def test_process_change_still_invalidates_confirmed_target():
    class Backend:
        def __init__(self): self.count=0
        async def call(self,name,args):
            self.count+=1
            return {'ok':True,'content':f'WhatsApp [net.whatsapp.WhatsApp] pid={self.count} frontmost=true windows=1\n  [e3] TextArea "Compose message"'}
    b=Backend(); g=Gateway(b,allowed_apps=('net.whatsapp.WhatsApp',))
    await g.dispatch('computer','inspect',{'app':'net.whatsapp.WhatsApp'})
    request=await g.dispatch('computer','type',{'ref':'e3','text':'hello'})
    assert (await g.dispatch('computer','confirm',{'token':request['token']}))['code']=='stale_ref'

@pytest.mark.anyio
async def test_visual_click_is_app_scoped_and_requires_confirmation():
    class Backend:
        def __init__(self): self.calls=[]
        async def call(self, name, args):
            self.calls.append((name,args))
            if name == 'get_app_state':
                return {'ok':True,'content':'Example [com.example.test] pid=123 windows=1\n── window 0: Example\n  [e1] Group "Canvas"'}
            return {'ok':True,'content':'clicked'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.test'})
    pending=await gateway.dispatch('desktop','visual_click',{'x':120,'y':200})
    assert pending['code']=='confirmation_required' and len(backend.calls)==1
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['ok'] and not result['completed']
    assert backend.calls[-1]==('click',{'app':'bundle:com.example.test','x':120,'y':200})

@pytest.mark.anyio
async def test_visual_click_refuses_secure_or_unverified_snapshots():
    class Backend:
        async def call(self,name,args):
            return {'ok':True,'content':'Example [com.example.test] pid=123 windows=1\n── window 0: Example\n  [e1] UnverifiedTextField'}
    gateway=Gateway(Backend(),allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.test'})
    result=await gateway.dispatch('desktop','visual_click',{'x':120,'y':200})
    assert not result['ok'] and result.get('code')!='confirmation_required'

@pytest.mark.anyio
@pytest.mark.parametrize('snapshot,args',[
    ('Example [com.example.test] pid=123 windows=2\n── window 0: Example\n  [e1] Group "Canvas"', {'app':'com.example.test'}),
    ('Example [com.example.test] pid=123 windows=1\n── window 0: Example\n  [e1] Group "Canvas"', {'app':'com.example.test','window':0}),
])
async def test_visual_click_refuses_incomplete_or_single_window_preflight(snapshot,args):
    class Backend:
        async def call(self,name,arguments): return {'ok':True,'content':snapshot}
    gateway=Gateway(Backend(),allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',args)
    result=await gateway.dispatch('desktop','visual_click',{'x':120,'y':200})
    assert result.get('code')!='confirmation_required'

@pytest.mark.anyio
async def test_visual_click_rejects_stale_state_before_emitting_input():
    class Backend:
        def __init__(self): self.changed=False; self.calls=[]
        async def call(self,name,args):
            self.calls.append(name)
            label='Changed' if self.changed else 'Canvas'
            return {'ok':True,'content':f'Example [com.example.test] pid=123 windows=1\n── window 0: Example\n  [e1] Group "{label}"'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.test'})
    pending=await gateway.dispatch('desktop','visual_click',{'x':1,'y':2})
    backend.changed=True
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert result['code']=='stale_ref' and result['input_emitted'] is False
    assert backend.calls == ['get_app_state','get_app_state']

@pytest.mark.anyio
@pytest.mark.parametrize('xy',[{'x':True,'y':2},{'x':float('nan'),'y':2},{'x':100001,'y':2}])
async def test_visual_click_rejects_invalid_coordinates(xy):
    class Backend:
        async def call(self,name,args): return {'ok':True,'content':'Example [com.example.test] pid=123 windows=1\n── window 0: Example\n  [e1] Group "Canvas"'}
    gateway=Gateway(Backend(),allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.test'})
    assert (await gateway.dispatch('desktop','visual_click',xy))['code'] != 'confirmation_required'

@pytest.mark.anyio
async def test_visual_click_denied_in_read_only_mode():
    gateway=Gateway(FakeBackend(),allowed_apps=['*'],mode='read_only')
    assert (await gateway.dispatch('desktop','visual_click',{'x':1,'y':2}))['code']=='read_only'

@pytest.mark.anyio
async def test_screenshot_checks_hidden_elements_privately_when_visible_tree_skips():
    class Backend:
        def __init__(self): self.hidden='  [e2] Button "Hidden nonsecret"'; self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            tail=self.hidden if args.get('offscreen') else '… 1 off-screen elements skipped; scroll, or pass offscreen=true to list them'
            return {'ok':True,'content':'App [com.example.editor] pid=1 frontmost=false windows=1\n── window 0: "Document"\n  [e1] Window "Document"\n'+tail}
        async def capture(self,args): self.calls.append(('capture',args)); return {'ok':True,'image':'image fixture'}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    result=await gateway.capture_image('com.example.editor')
    assert result['ok'] and result['image']=='image fixture'
    assert any(args.get('offscreen') is True for name,args in backend.calls if name=='get_app_state')
    assert 'Hidden nonsecret' not in repr(result)
    backend.hidden='  [e2] SecureTextField'
    result=await gateway.capture_image('com.example.editor')
    assert not result['ok'] and 'SecureTextField' not in repr(result)

@pytest.mark.anyio
async def test_visual_click_checks_hidden_elements_again_on_confirmation():
    class Backend:
        def __init__(self): self.hidden='  [e2] Button "Hidden"'; self.calls=[]
        async def call(self,name,args):
            self.calls.append((name,args))
            if name != 'get_app_state': return {'ok':True,'content':'clicked'}
            tail=self.hidden if args.get('offscreen') else '… 1 off-screen elements skipped; scroll, or pass offscreen=true to list them'
            return {'ok':True,'content':'App [com.example.editor] pid=1 windows=1\n── window 0: "Document"\n  [e1] Window "Document"\n'+tail}
    backend=Backend(); gateway=Gateway(backend,allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    pending=await gateway.dispatch('desktop','visual_click',{'x':100,'y':200})
    assert pending['code']=='confirmation_required'
    backend.hidden='  [e2] SecureTextField'
    result=await gateway.dispatch('computer','confirm',{'token':pending['token']})
    assert not result['ok'] and result['input_emitted'] is False
    assert all(name != 'click' for name,args in backend.calls)

@pytest.mark.anyio
@pytest.mark.parametrize('full_tail,full_pid',[
    ('  [e2] UnverifiedTextField', 1),
    ('… truncated due to limit', 1),
    ('  [e2] Button "Different process"', 2),
])
async def test_screenshot_rejects_unsafe_hidden_preflight_or_changed_pid(full_tail,full_pid):
    class Backend:
        async def call(self,name,args):
            pid=full_pid if args.get('offscreen') else 1
            tail=full_tail if args.get('offscreen') else '… 1 off-screen elements skipped; scroll, or pass offscreen=true to list them'
            return {'ok':True,'content':f'App [com.example.editor] pid={pid} windows=1\n── window 0: "Document"\n  [e1] Window "Document"\n{tail}'}
        async def capture(self,args): raise AssertionError('Unsafe preflight must not capture')
    result=await Gateway(Backend(),allowed_apps=['*']).capture_image('com.example.editor')
    assert not result['ok'] and result['code']=='unsafe_or_incomplete_tree'

@pytest.mark.anyio
@pytest.mark.parametrize('error,code,emitted',[
    ('error: coordinate is outside the selected app window; no input was sent','coordinate_outside_window',False),
    ('error: accessibility press failed; inspect state before retrying','accessibility_press_failed',None),
])
async def test_visual_click_reports_specific_safe_native_failure(error,code,emitted):
    class Backend:
        async def call(self,name,args):
            if name=='get_app_state': return {'ok':True,'content':'App [com.example.editor] pid=1 windows=1\n── window 0: "Document"\n  [e1] Window "Document"'}
            return {'ok':False,'content':error}
    gateway=Gateway(Backend(),allowed_apps=['*'])
    await gateway.dispatch('computer','inspect',{'app':'com.example.editor'})
    request=await gateway.dispatch('desktop','visual_click',{'x':100,'y':200})
    result=await gateway.dispatch('computer','confirm',{'token':request['token']})
    assert result['code']==code and result['input_emitted'] is emitted

@pytest.mark.anyio
async def test_inspection_reports_incomplete_tree_without_backend_details():
    class Backend:
        async def call(self, name, args):
            return {'ok': False, 'code': 'unsafe_or_incomplete_tree', 'content':'synthetic-secret'}
    result = await Gateway(Backend()).dispatch('computer','inspect',{'app':'Calculator'})
    assert result['code'] == 'unsafe_or_incomplete_tree'
    assert 'synthetic-secret' not in repr(result)

def test_chrome_tab_memory_usage_changes_do_not_stale_snapshot():
    from handsmcp.gateway import comparable_snapshot
    before = 'Google Chrome [com.google.Chrome] pid=1 frontmost=false windows=1\n  [e2] RadioButton "Test Page - Memory usage - 20.3 MB" value="1"'
    after = before.replace('20.3 MB', '20.1 MB')
    assert comparable_snapshot(before) == comparable_snapshot(after)

def test_chrome_tab_title_changes_still_stale_snapshot():
    from handsmcp.gateway import comparable_snapshot
    before = 'Google Chrome [com.google.Chrome] pid=1 frontmost=false windows=1\n  [e2] RadioButton "Test Page - Memory usage - 20.3 MB" value="1"'
    after = before.replace('Test Page', 'Different Page')
    assert comparable_snapshot(before) != comparable_snapshot(after)

def test_non_chrome_memory_label_change_still_stales_snapshot():
    from handsmcp.gateway import comparable_snapshot
    before = 'App [com.example.app] pid=1 frontmost=false windows=1\n  [e2] RadioButton "Test Page - Memory usage - 20.3 MB" value="1"'
    assert comparable_snapshot(before) != comparable_snapshot(before.replace('20.3 MB', '20.1 MB'))

def test_safe_replacement_error_categories_do_not_echo_upstream_text():
    from handsmcp.gateway import replacement_error_code
    assert replacement_error_code('error: focused target could not be reverified; no text was sent') == 'focus_revalidation_failed'
    assert replacement_error_code('error: field value could not be verified after replacement; inspect before retrying') == 'replacement_unverified'
    assert replacement_error_code('sensitive arbitrary error') == 'upstream_error'

@pytest.mark.anyio
async def test_confirmed_replacement_exposes_only_fixed_error_code():
    class Backend:
        async def call(self, name, args):
            if name == 'get_app_state':
                return {'ok':True,'content':'Chrome [com.google.Chrome] pid=1 frontmost=false windows=1\n  [e3] TextField "Practice text"'}
            return {'ok':False,'content':'error: focused target could not be reverified; no text was sent'}
    g=Gateway(Backend(),allowed_apps=['*'])
    await g.dispatch('computer','inspect',{'app':'com.google.Chrome'})
    request=await g.dispatch('computer','type',{'ref':'e3','text':'test','mode':'replace'})
    result=await g.dispatch('computer','confirm',{'token':request['token']})
    assert result['code']=='focus_revalidation_failed' and 'content' not in result
