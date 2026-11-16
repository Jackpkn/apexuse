import asyncio
import json
from types import SimpleNamespace
from handsmcp.cua_client import CuaClient

class Session:
    def __init__(self, value): self.value=value; self.calls=[]
    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        return self.value

def result(data=None, error=False):
    return SimpleNamespace(structuredContent=data, isError=error, content=[])

def call(client, name, args): return asyncio.run(client.call(name, args))

def test_inventory_keeps_only_running_app_bundle_and_pid():
    session=Session(result({'apps':[
        {'bundle_id':'com.apple.calculator','pid':123,'running':True,'name':'Private title'},
        {'bundle_id':'com.example.off','pid':0,'running':False},
        {'bundle_id':'com.example.unknown','pid':55,'running':True},
    ]}))
    value=call(CuaClient(session), 'list_apps', {})
    assert value['ok'] is True
    assert json.loads(value['content']) == [{'bundle_id':'com.apple.calculator','pid':123},{'bundle_id':'com.example.unknown','pid':55}]
    assert session.calls == [('list_apps', {})]

def test_unexpected_or_invalid_inventory_is_rejected():
    for data in (None, {'apps':'bad'}, {'apps':[{'bundle_id':'com.test.app','pid':True,'running':True}]}, {'refusal':{'code':'denied'}}):
        assert not call(CuaClient(Session(result(data))), 'list_apps', {})['ok']
    assert not call(CuaClient(Session(result({'apps':[]},error=True))), 'list_apps', {})['ok']

def test_other_operations_are_never_forwarded():
    session=Session(result({'apps':[]})); client=CuaClient(session)
    for name in ('click','type_text','get_app_state','press_key'):
        assert not call(client,name,{'unsafe':'input'})['ok']
    assert session.calls == []

def test_incomplete_cua_snapshot_is_not_exposed():
    class Scripted:
        async def call_tool(self, name, arguments):
            if name == 'list_apps': return result({'apps':[{'bundle_id':'com.example.app','pid':42,'running':True}]})
            if name == 'list_windows': return result({'windows':[{'pid':42,'window_id':11}]})
            if name == 'get_window_state': return result({'pid':42,'window_id':11,'snapshot_id':'s00000001','elements_complete':False,'truncated':False,'elements':[{'role':'AXTextField','value':'synthetic-secret'}]})
            raise AssertionError(name)
    value=call(CuaClient(Scripted()), 'get_app_state', {'app':'bundle:com.example.app'})
    assert value['ok'] is False
    assert value['code'] == 'unsafe_or_incomplete_tree'
    assert 'synthetic-secret' not in str(value)

def test_secure_subrole_rejected_even_if_snapshot_claims_complete():
    class Scripted:
        async def call_tool(self, name, arguments):
            if name == 'list_apps': return result({'apps':[{'bundle_id':'com.example.app','pid':42,'running':True}]})
            if name == 'list_windows': return result({'windows':[{'pid':42,'window_id':11}]})
            if name == 'get_window_state': return result({'pid':42,'window_id':11,'snapshot_id':'s00000001','elements_complete':True,'truncated':False,'elements':[{'role':'AXTextField','subrole':'AXSecureTextField','value':'synthetic-secret'}]})
            raise AssertionError(name)
    value=call(CuaClient(Scripted()), 'get_app_state', {'app':'bundle:com.example.app'})
    assert not value['ok'] and value['code']=='unsafe_or_incomplete_tree'
    assert 'synthetic-secret' not in str(value)

def test_complete_safe_snapshot_can_be_inspected_without_input():
    class Scripted:
        async def call_tool(self, name, arguments):
            if name=='list_apps': return result({'apps':[{'bundle_id':'com.example.app','pid':42,'running':True}]})
            if name=='list_windows': return result({'windows':[{'pid':42,'window_id':11}]})
            if name=='get_window_state': return result({'pid':42,'window_id':11,'snapshot_id':'s00000001','elements_complete':True,'truncated':False,'elements':[{'role':'AXButton','label':'Apply','element_token':'s00000001:0'}]})
            raise AssertionError(name)
    value=call(CuaClient(Scripted()), 'get_app_state', {'app':'bundle:com.example.app'})
    assert value['ok'] and '[com.example.app] pid=42' in value['content']
    assert 'Button "Apply"' in value['content']
