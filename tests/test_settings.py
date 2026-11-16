import pytest
from handsmcp.settings import load_settings

def test_loads_valid_safe_settings(tmp_path):
    path=tmp_path/'policy.yaml'
    path.write_text('mode: read_only\nrate_limit_per_minute: 4\nconfirmation_ttl_seconds: 45\n')
    cfg=load_settings(path)
    assert cfg.mode=='read_only' and cfg.rate_limit_per_minute==4 and cfg.confirmation_ttl_seconds==45

def test_rejects_bad_policy_mode(tmp_path):
    path=tmp_path/'policy.yaml'; path.write_text('mode: unrestricted\n')
    with pytest.raises(ValueError): load_settings(path)

def test_rejects_unknown_fields(tmp_path):
    path=tmp_path/'policy.yaml'; path.write_text('unexpected: true\n')
    with pytest.raises(ValueError): load_settings(path)

def test_custom_app_bundle_allowlist(tmp_path):
    path=tmp_path/'policy.yaml'
    path.write_text('allowed_apps:\n  - com.apple.TextEdit\n  - com.example.safe\n')
    assert 'com.example.safe' in load_settings(path).allowed_apps

def test_protected_app_cannot_be_allowlisted(tmp_path):
    path=tmp_path/'policy.yaml'
    path.write_text('allowed_apps:\n  - com.1password.1password\n')
    with pytest.raises(ValueError): load_settings(path)

def test_apple_passwords_is_protected(tmp_path):
    path=tmp_path/'policy.yaml'; path.write_text('allowed_apps:\n  - com.apple.Passwords\n')
    with pytest.raises(ValueError): load_settings(path)

def test_all_apps_can_be_enabled_explicitly(tmp_path):
    path=tmp_path/'policy.yaml'; path.write_text('allowed_apps: ["*"]\n')
    assert load_settings(path).allowed_apps == ('*',)
