import json
import os
import stat
import pytest
from handsmcp.audit import AuditLog

def test_records_only_safe_fields_and_no_input_text(tmp_path):
    path = tmp_path / 'audit.jsonl'
    audit = AuditLog(path)
    audit.record(action='type', decision='deny', code='secure_field', text='do not log this', target='r12')
    line = json.loads(path.read_text())
    assert line['action'] == 'type' and line['decision'] == 'deny'
    assert line['code'] == 'secure_field'
    assert 'do not log this' not in path.read_text()
    assert stat.S_IMODE(path.stat().st_mode) == 0o600

def test_symlink_log_is_refused(tmp_path):
    target = tmp_path / 'other'
    target.write_text('unchanged')
    path = tmp_path / 'audit.jsonl'
    path.symlink_to(target)
    with pytest.raises(OSError):
        AuditLog(path).record(action='click', decision='deny', code='blocked')
    assert target.read_text() == 'unchanged'

def test_unrecognized_action_and_code_are_not_logged_verbatim(tmp_path):
    path = tmp_path / 'audit.jsonl'
    AuditLog(path).record(action='SecretValue123', decision='deny', code='SecretValue456')
    assert 'SecretValue' not in path.read_text()
    assert json.loads(path.read_text())['action'] == 'invalid'
