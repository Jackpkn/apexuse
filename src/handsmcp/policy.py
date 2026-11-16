"""Conservative policy primitives; not an enforcement boundary on their own."""
from enum import Enum

class Decision(str, Enum):
    ALLOW = 'allow'
    DENY = 'deny'
    CONFIRM = 'confirm'

INPUT = frozenset({'click', 'double_click', 'right_click', 'type', 'press_key', 'scroll', 'drag', 'select', 'focus', 'visual_click', 'visual_scroll'})
READS = frozenset({'inspect', 'get_ui_tree', 'get_interactive_elements', 'screenshot', 'zoom', 'list_applications', 'list_windows', 'window_information'})
DENIED = frozenset({'1password', 'bitwarden', 'keychain access', 'dashlane', 'lastpass'})

def classify(action, target, mode='normal'):
    if mode not in {'normal', 'read_only', 'dry_run'}:
        return Decision.DENY
    if str(target.get('app', '')).casefold() in DENIED:
        return Decision.DENY
    if action in INPUT:
        if mode in {'read_only', 'dry_run'}:
            return Decision.DENY
        if action == 'type' and (target.get('secure') is not False or not target.get('app')):
            return Decision.DENY
        return Decision.CONFIRM
    return Decision.ALLOW if action in READS else Decision.DENY
