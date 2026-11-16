import pytest
from handsmcp.policy import Decision, classify

@pytest.mark.parametrize('action', ['click', 'type', 'press_key', 'drag', 'visual_click'])
def test_read_only_blocks_input(action):
    assert classify(action, {}, mode='read_only') == Decision.DENY

def test_typing_requires_known_non_secure_target():
    assert classify('type', {}, mode='normal') == Decision.DENY
    assert classify('type', {'secure': True}, mode='normal') == Decision.DENY
    assert classify('type', {'secure': False, 'app': 'TextEdit'}, mode='normal') == Decision.CONFIRM

def test_sensitive_target_requires_confirmation():
    assert classify('click', {'name': 'Send message', 'app': 'Messages'}, mode='normal') == Decision.CONFIRM

def test_unclassified_visual_click_requires_confirmation():
    assert classify('visual_click', {}, mode='normal') == Decision.CONFIRM

def test_denied_app_cannot_be_inspected():
    assert classify('inspect', {'app': '1Password'}, mode='normal') == Decision.DENY

def test_unknown_action_is_denied():
    assert classify('unknown_action', {}, mode='normal') == Decision.DENY
