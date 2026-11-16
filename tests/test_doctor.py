import json
from unittest.mock import patch
from handsmcp.doctor import check_cua

def test_missing_driver_is_reported_without_starting_anything():
    with patch('handsmcp.doctor.shutil.which', return_value=None), patch('handsmcp.doctor.Path.is_file', return_value=False), patch('handsmcp.doctor.subprocess.run') as run:
        result = check_cua()
    assert result['status'] == 'not_installed'
    run.assert_not_called()

def test_granted_permissions_report_ready():
    payload = {'accessibility': True, 'screen_recording': True}
    with patch('handsmcp.doctor.shutil.which', return_value='/opt/cua-driver'), patch('handsmcp.doctor.subprocess.run') as run:
        run.return_value.returncode = 0
        run.return_value.stdout = json.dumps(payload)
        result = check_cua()
    assert result['status'] == 'ready'
    assert result['accessibility'] is True
    assert result['screen_recording'] is True
    assert run.call_args.args[0] == ['/opt/cua-driver', 'permissions', 'status', '--json']

def test_false_or_unknown_permissions_are_not_ready():
    with patch('handsmcp.doctor.shutil.which', return_value='/opt/cua-driver'), patch('handsmcp.doctor.subprocess.run') as run:
        run.return_value.returncode = 0
        run.return_value.stdout = json.dumps({'accessibility': True, 'screen_recording': False})
        assert check_cua()['status'] == 'permissions_needed'
        run.return_value.stdout = json.dumps({'status': 'unknown'})
        assert check_cua()['status'] == 'unknown'

def test_bad_output_is_not_ready():
    with patch('handsmcp.doctor.shutil.which', return_value='/opt/cua-driver'), patch('handsmcp.doctor.subprocess.run') as run:
        run.return_value.returncode = 0
        run.return_value.stdout = 'not json'
        assert check_cua()['status'] == 'unknown'
