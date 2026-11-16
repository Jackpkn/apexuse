"""Non-destructive checks for optional computer-use dependencies."""
import json
import os
from pathlib import Path
import shutil
import subprocess


def check_cua():
    command = shutil.which('cua-driver')
    if not command:
        app_binary = Path('/Applications/CuaDriver.app/Contents/MacOS/cua-driver')
        if app_binary.is_file():
            command = str(app_binary)
    if not command:
        return {'status': 'not_installed'}
    try:
        completed = subprocess.run(
            [command, 'permissions', 'status', '--json'],
            capture_output=True, text=True, timeout=20, check=False,
            env={**os.environ, 'DO_NOT_TRACK': '1'},
        )
        if completed.returncode != 0:
            return {'status': 'unknown'}
        data = json.loads(completed.stdout)
        accessibility = data.get('accessibility')
        screen_recording = data.get('screen_recording')
        if type(accessibility) is not bool or type(screen_recording) is not bool:
            return {'status': 'unknown'}
        return {'status': 'ready' if accessibility and screen_recording else 'permissions_needed',
                'accessibility': accessibility, 'screen_recording': screen_recording}
    except (OSError, subprocess.TimeoutExpired, ValueError, AttributeError):
        return {'status': 'unknown'}
