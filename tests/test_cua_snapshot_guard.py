from handsmcp.cua_client import safe_snapshot

def snapshot(elements=None):
    return {'elements_complete':True, 'truncated':False, 'elements':elements or []}

def test_complete_plain_snapshot_is_accepted():
    assert safe_snapshot(snapshot([{'role':'AXButton','label':'Apply'}]))

def test_missing_incomplete_and_truncated_snapshots_fail_closed():
    for item in ({}, {'elements_complete':False,'truncated':False,'elements':[]}, {'elements_complete':True,'truncated':True,'elements':[]}, {'elements_complete':True,'truncated':False,'elements':'bad'}):
        assert not safe_snapshot(item)

def test_secure_field_role_or_subrole_rejected():
    for field in ({'role':'AXSecureTextField'}, {'role':'AXTextField','subrole':'AXSecureTextField'}, {'role':'AXTextField','subrole':'axsecuretextfield'}):
        assert not safe_snapshot(snapshot([field]))

def test_malformed_unknown_roles_and_protected_markers_fail_closed():
    for field in (None, {'role':12}, {'role':'AXTextField','value':'••••'}, {'role':'AXTextField','description':'Password'}):
        assert not safe_snapshot(snapshot([field]))
