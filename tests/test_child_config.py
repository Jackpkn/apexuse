from handsmcp.server import child_parameters

def test_child_secure_input_override_cannot_be_inherited(monkeypatch, tmp_path):
    monkeypatch.setenv('COMPUTER_USE_ALLOW_SECURE_FIELD_INPUT','1')
    params = child_parameters(str(tmp_path / 'runtime'))
    assert params.env['COMPUTER_USE_ALLOW_SECURE_FIELD_INPUT'] == '0'
    assert params.args == []

def test_agent_pointer_is_enabled_for_desktop_actions(tmp_path):
    params = child_parameters(str(tmp_path / 'runtime'))
    assert params.env['COMPUTER_USE_AGENT_CURSOR'] == '1'
    assert params.env['COMPUTER_USE_AGENT_CURSOR_TASK_FADE_SECS'] == '8'

def test_child_disables_unrequested_browser_bridge(tmp_path):
    params = child_parameters(str(tmp_path / 'runtime'))
    assert params.env['HANDSMCP_DISABLE_BROWSER_BRIDGE'] == '1'

def test_cua_child_uses_stdio_and_disables_telemetry(monkeypatch):
    from handsmcp.server import backend_parameters
    monkeypatch.setenv('HANDSMCP_BACKEND', 'cua')
    monkeypatch.setenv('HANDSMCP_CUA', '/opt/cua driver')
    kind, params = backend_parameters('/unused')
    assert kind == 'cua'
    assert params.command == '/opt/cua driver'
    assert params.args == ['mcp']
    assert params.env['DO_NOT_TRACK'] == '1'

def test_unknown_backend_fails_closed(monkeypatch):
    import pytest
    from handsmcp.server import backend_parameters
    monkeypatch.setenv('HANDSMCP_BACKEND', 'typo')
    with pytest.raises(ValueError): backend_parameters('/unused')
