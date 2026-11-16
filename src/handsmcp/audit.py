"""Privacy-minimizing, append-only event records."""
from datetime import datetime, timezone
import json
import os
from pathlib import Path

_ACTIONS = {'inspect','get_ui_tree','get_interactive_elements','click','double_click','right_click','type','press_key','scroll','drag','select','focus','confirm','screenshot','zoom','list_applications','list_windows','window_information','activate_application','launch_application','visual_click','visual_scroll'}
_DECISIONS = {'allow','deny','confirm','confirmed'}
_CODES = {'requested','not_implemented','confirmation_required','secure_field','blocked'}

def _safe(value, allowed):
    return value if isinstance(value, str) and value in allowed else 'invalid'

class AuditLog:
    def __init__(self, path): self.path = Path(path)

    def record(self, *, action, decision, code, **_ignored):
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        entry = {'time': datetime.now(timezone.utc).isoformat(), 'action': _safe(action, _ACTIONS),
                 'decision': _safe(decision, _DECISIONS), 'code': _safe(code, _CODES)}
        data = (json.dumps(entry, separators=(',', ':')) + '\n').encode('utf-8')
        flags = os.O_WRONLY | os.O_CREAT | os.O_APPEND | getattr(os, 'O_NOFOLLOW', 0)
        fd = os.open(self.path, flags, 0o600)
        try:
            os.fchmod(fd, 0o600)
            if os.write(fd, data) != len(data):
                raise OSError('incomplete audit record')
        finally:
            os.close(fd)
