"""Fail-closed gateway with limited inspection and confirmed Calculator input."""
import asyncio
import json
from contextlib import asynccontextmanager
from pathlib import Path
import re
import time
import unicodedata
from .confirm import ConfirmationStore
from .rate_limit import RateLimiter
from .settings import DEFAULT_APPS, validate_apps, valid_bundle
from .app_launch import launch_application

_APP_ALIASES = {'TextEdit':'com.apple.TextEdit', 'Calculator':'com.apple.calculator'}
class _LocalLease:
    def generation(self): return 0
    def advance(self): pass

@asynccontextmanager
async def _local_hold():
    yield _LocalLease()

_SAFE_BUTTONS = set('0123456789') | {'Add', 'Subtract', 'Multiply', 'Divide', 'Equals', 'Point', 'Clear', 'All Clear'}
_SECURE_LINE = re.compile(r'^(\s*)(\[e\d+\]\s*)?SecureTextField\b', re.IGNORECASE)
_AREA_LINE = re.compile(r'^\s*\[(e\d+)\]\s+(TextArea|TextField|SearchField)\b')
_BUTTON_LINE = re.compile(r'^\s*\[(e\d+)\]\s+Button "([^"\n]+)"(?:\s|$)')
_NAVIGATION_KEYS = frozenset({'return', 'enter', 'tab', 'escape', 'esc', 'up', 'down', 'left', 'right', 'home', 'end', 'pageup', 'pagedown', 'delete', 'backspace', 'forwarddelete'} | {f'f{i}' for i in range(1, 21)})
_KEY_MODIFIERS = frozenset({'cmd', 'ctrl', 'alt', 'shift'})

def valid_key_request(arguments):
    if set(arguments) != {'key', 'modifiers'}:
        return False
    key, mods = arguments['key'], arguments['modifiers']
    if not isinstance(key, str) or not isinstance(mods, list) or any(not isinstance(m, str) or m not in _KEY_MODIFIERS for m in mods) or len(mods) != len(set(mods)):
        return False
    if key.lower() == 'v' and {'cmd', 'ctrl'} & set(mods):
        return False  # Clipboard content cannot be checked by the text-field safeguards.
    return key in _NAVIGATION_KEYS or (len(key) == 1 and key.isascii() and key.isalnum() and bool({'cmd', 'ctrl'} & set(mods)))

def whatsapp_contact_refs(content):
    # Only an unfiltered New chat sheet with contact-section headings qualifies.
    plain = ''.join(c for c in content if unicodedata.category(c) != 'Cf')
    if not re.search(r'\[e\d+\] Sheet\b', plain) or not re.search(r'Heading "New chat"', plain, re.I):
        return set()
    if not re.search(r'Heading "(?:Frequently contacted|Contacts on WhatsApp)"', plain, re.I):
        return set()
    refs = set()
    lines = content.splitlines()
    section = False
    group = None
    for line in lines:
        clean = ''.join(c for c in line if unicodedata.category(c) != 'Cf')
        if re.search(r'Heading "(?:Frequently contacted|Contacts on WhatsApp)"', clean, re.I):
            section = True
        g = re.match(r'^(\s*)\[e\d+\] Group "([^"\n]+)"', line)
        if g:
            group = (len(g.group(1)), g.group(2)) if section else None
            continue
        b = re.match(r'^(\s*)\[(e\d+)\] Button "([^"\n]+)"', line)
        if b and group and len(b.group(1)) > group[0] and b.group(3) == group[1]:
            if not re.search(r'\b(?:send|delete|remove|pay|buy|publish|submit|transfer)\b', b.group(3), re.I):
                refs.add(b.group(2))
        if line.strip() and group and len(line) - len(line.lstrip()) <= group[0]:
            group = None
    return refs

def replacement_error_code(message):
    # Only expose fixed categories, never application content or field values.
    known = {
        'error: requested text field identity could not be verified': 'target_identity_unverified',
        'error: could not focus the requested field; no text was sent': 'focus_failed',
        'error: focused target could not be reverified; no text was sent': 'focus_revalidation_failed',
        'error: field value could not be verified after replacement; inspect before retrying': 'replacement_unverified',
    }
    if isinstance(message, str) and re.fullmatch(r'error: could not set value on e[0-9]+ \(AX error -?[0-9]+\); try click \+ type_text instead', message):
        return 'replacement_set_failed'
    return known.get(message, 'upstream_error') if isinstance(message, str) else 'upstream_error'

def comparable_snapshot(content):
    if not isinstance(content, str):
        return None
    first, separator, rest = content.partition('\n')
    first = re.sub(r'(?<= )frontmost=(?:true|false)(?= |$)', 'frontmost=<state>', first, count=1)
    if '[com.google.Chrome]' in first:
        # Chrome updates tab memory telemetry even when the actual target is unchanged.
        # Retain the tab title, selected state, all other nodes and every other app.
        rest = re.sub(r'(?m)^(\s*\[e\d+\] RadioButton "[^"\n]+ - (?:High )?memory usage - )\d+(?:\.\d+)? [KMGT]B(?="(?:\s|$))', r'\g<1><memory>', rest, flags=re.IGNORECASE)
    return first + separator + rest

def contains_secure_marker(content):
    # A spaced bullet in static text is a common time/message separator.
    # Keep masks in fields, values, and every other context fail-closed.
    checked = []
    for line in content.splitlines():
        if re.match(r'^\s*(?:\[e\d+\]\s*)?StaticText\b', line) and 'value=' not in line:
            line = re.sub(r'(?<=\s)•(?=\s)', '', line)
        checked.append(line)
    return bool(re.search(r'(?:secure.?text.?field|password.?field|protected.?content|[\uf79a\u2022\u25cf]+)', '\n'.join(checked), re.IGNORECASE))

def redact_secure_fields(content):
    # Known secure markers always suppress the entire response.
    if contains_secure_marker(content):
        return '[redacted: secure field detected]'
    lines = content.split('\n')
    out = []
    index = 0
    while index < len(lines):
        line = lines[index]
        if 'UnverifiedTextField' not in line:
            out.append(line)
            index += 1
            continue
        if not re.fullmatch(r'\s*(?:\[e\d+\]\s*)?UnverifiedTextField(?:\s+\((?:focused|disabled)\))*\s*', line):
            return '[redacted: secure field detected]'
        out.append(line)
        indent = len(line) - len(line.lstrip(' '))
        index += 1
        while index < len(lines):
            child = lines[index]
            if not child.strip():
                index += 1
                continue
            if len(child) - len(child.lstrip(' ')) <= indent:
                break
            # Only recognisable rendered nodes can be safely pruned. An
            # unstructured continuation may contain a field value.
            if not re.match(r'^\s*(?:\[e\d+\]\s*)?[A-Z][A-Za-z0-9]*\b', child):
                return '[redacted: secure field detected]'
            index += 1
    return '\n'.join(out)

def redaction_reason(content):
    # These fixed categories never include a field's label, value or app text.
    if re.search(r'secure.?text.?field', content, re.I):
        return 'secure_field_marker'
    if contains_secure_marker(content):
        return 'sensitive_text_marker'
    if 'UnverifiedTextField' in content:
        return 'unverified_field_content'
    return 'protected_content'

def unverified_field_diagnostic(content):
    lines = content.splitlines()
    nonempty = False
    has_descendants = False
    for index, line in enumerate(lines):
        if 'UnverifiedTextField' not in line:
            continue
        if not re.fullmatch(r'\s*(?:\[e\d+\]\s*)?UnverifiedTextField(?:\s+\((?:focused|disabled)\))*\s*',line):
            nonempty = True
        indent = len(line) - len(line.lstrip(' '))
        following = next((item for item in lines[index+1:] if item.strip()),None)
        if following is not None and len(following) - len(following.lstrip(' ')) > indent:
            has_descendants = True
    return {'nonempty_field_line':nonempty, 'has_descendants':has_descendants}

def complete_visual_snapshot(raw, app, args):
    if not isinstance(raw, str) or not isinstance(args, dict) or 'window' in args or 'query' in args:
        return False
    header = re.match(r'^[^\n]*\[' + re.escape(app) + r'\]\s+pid=\d+\b[^\n]*\bwindows=(\d+)\b', raw)
    if not header:
        return False
    count = int(header.group(1))
    indices = re.findall(r'^── window (\d+):',raw,re.M)
    return (1 <= count <= 20 and indices == [str(i) for i in range(count)]
            and not contains_secure_marker(raw) and 'UnverifiedTextField' not in raw
            and not re.search(r'\b(?:skipped|truncated)\b',raw,re.I))

class Gateway:
    def __init__(self, backend, audit=None, stop_file=None, desktop_lock=None, mode="normal", rate_limit=120, confirmation_ttl=120, allowed_apps=DEFAULT_APPS, app_launcher=None):
        self.backend = backend
        self.app_launcher = app_launcher or launch_application
        self.audit = audit
        self.stop_file = Path(stop_file) if stop_file else None
        self.lock = asyncio.Lock()
        self.desktop_lock = desktop_lock
        self.snapshot_generation = None
        self.snapshot_raw = None
        self.snapshot_app = None
        self.snapshot_args = None
        if mode not in {'normal', 'read_only', 'dry_run'}:
            raise ValueError('invalid mode')
        self.mode = mode
        self.allowed_apps = validate_apps(allowed_apps)
        self.confirmations = ConfirmationStore(ttl_seconds=confirmation_ttl)
        self.rate_limiter = RateLimiter(limit=rate_limit)
        self.epoch = 0
        self.buttons = {}
        self.textareas = {}
        self.contact_refs = set()

    def _audit(self, action, decision, code):
        try:
            if self.audit:
                self.audit.record(action=action, decision=decision, code=code)
            return True
        except OSError:
            return False

    async def _complete_visual_preflight(self, raw, app, args):
        if complete_visual_snapshot(raw, app, args):
            return True
        # The visible outline may omit off-screen subtrees. Recheck privately;
        # never return the hidden outline or persist it in the snapshot.
        if not isinstance(raw, str):
            return False
        without_skip = re.sub(r'^… \d+ off-screen elements skipped; scroll, or pass offscreen=true to list them\s*$', '', raw, flags=re.M)
        if without_skip == raw or not complete_visual_snapshot(without_skip, app, args):
            return False
        identity = re.match(r'^[^\n]*\[' + re.escape(app) + r'\]\s+pid=(\d+)\b[^\n]*\bwindows=(\d+)\b', raw)
        if not identity:
            return False
        full_args = dict(args, offscreen=True)
        try:
            result = await self.backend.call('get_app_state', full_args)
        except Exception:
            return False
        full = result.get('content')
        full_identity = re.match(r'^[^\n]*\[' + re.escape(app) + r'\]\s+pid=(\d+)\b[^\n]*\bwindows=(\d+)\b', full) if isinstance(full, str) else None
        return bool(result.get('ok') and full_identity and full_identity.groups() == identity.groups()
                    and complete_visual_snapshot(full, app, args))

    async def capture_image(self, app):
        bundle = _APP_ALIASES.get(app, app) if isinstance(app, str) else None
        if not valid_bundle(bundle) or not (bundle in self.allowed_apps or '*' in self.allowed_apps):
            return {'ok':False, 'code':'invalid_app'}
        async with self.lock:
            try:
                context = self.desktop_lock.hold() if self.desktop_lock else _local_hold()
                async with context:
                    if self.stop_file and self.stop_file.exists():
                        return {'ok':False, 'code':'panic_stop'}
                    if not self.rate_limiter.allow():
                        return {'ok':False, 'code':'rate_limited'}
                    if not self._audit('screenshot','allow','requested'):
                        return {'ok':False, 'code':'audit_unavailable'}
                    # The native inspection refreshes its element registry.
                    self.epoch += 1
                    self.snapshot_raw = None
                    self.buttons = {}
                    self.textareas = {}
                    result = await self.backend.call('get_app_state', {'app':'bundle:'+bundle, 'max_elements':150, 'max_depth':12, 'ocr':'never'})
                    raw = result.get('content')
                    if not result.get('ok') or not isinstance(raw,str):
                        return {'ok':False, 'code':'unsafe_or_ambiguous_target'}
                    header = re.match(r'^[^\n]*\[' + re.escape(bundle) + r'\]\s+pid=\d+\b[^\n]*\bwindows=(\d+)\b',raw)
                    if not header:
                        return {'ok':False, 'code':'unsafe_or_ambiguous_target'}
                    count = int(header.group(1))
                    indices = re.findall(r'^── window (\d+):',raw,re.M)
                    if count < 1 or count > 20 or indices != [str(i) for i in range(count)]:
                        return {'ok':False, 'code':'unsafe_or_ambiguous_target'}
                    if not await self._complete_visual_preflight(raw, bundle, {'app':'bundle:'+bundle, 'max_elements':150, 'max_depth':12, 'ocr':'never'}):
                        return {'ok':False, 'code':'unsafe_or_incomplete_tree'}
                    return await self.backend.capture({'app':'bundle:'+bundle})
            except (OSError, TimeoutError):
                return {'ok':False, 'code':'desktop_lock_unavailable'}
            except Exception:
                return {'ok':False, 'code':'upstream_error'}

    async def dispatch(self, tool, action, arguments):
        start = time.monotonic()
        result = await self._dispatch(tool, action, arguments)
        result.setdefault('input_emitted', False)
        result.setdefault('completed', bool(result.get('ok')) and action != 'confirm')
        result.setdefault('changed', None)
        result['elapsed_ms'] = round((time.monotonic() - start) * 1000, 2)
        result['policy'] = ('confirm' if result.get('code') == 'confirmation_required' else
                            'confirmed' if action == 'confirm' and result['input_emitted'] is None else
                            'allow' if result.get('ok') else 'deny')
        return result

    async def _dispatch(self, tool, action, arguments):
        async with self.lock:
            try:
                context = self.desktop_lock.hold() if self.desktop_lock else _local_hold()
                async with context as lease:
                    return await self._dispatch_locked(tool, action, arguments, lease)
            except (OSError, TimeoutError):
                return {'ok': False, 'code': 'desktop_lock_unavailable', 'input_emitted': False}

    async def _dispatch_locked(self, tool, action, arguments, lease):
            if self.stop_file and self.stop_file.exists():
                return {'ok': False, 'code': 'panic_stop', 'input_emitted': False}
            if not self.rate_limiter.allow():
                return {'ok': False, 'code': 'rate_limited', 'input_emitted': False}
            if not isinstance(arguments, dict):
                return {'ok': False, 'code': 'invalid_argument', 'input_emitted': False}
            if self.mode != 'normal' and ((tool == 'computer' and action in {'click', 'type', 'press_key', 'scroll', 'confirm'}) or (tool == 'desktop' and action == 'visual_click')):
                return {'ok': False, 'code': self.mode, 'input_emitted': False}
            inventory = tool == 'desktop' and action == 'list_applications' and not arguments
            requested_app = arguments.get('app')
            bundle = _APP_ALIASES.get(requested_app, requested_app) if isinstance(requested_app, str) else None
            if (tool == 'desktop' and action == 'launch_application' and set(arguments) == {'app'}
                    and valid_bundle(bundle) and (bundle in self.allowed_apps or '*' in self.allowed_apps)):
                if self.mode != 'normal':
                    return {'ok':False, 'code':self.mode, 'input_emitted':False}
                if not self._audit(action,'allow','requested'):
                    return {'ok':False, 'code':'audit_unavailable', 'input_emitted':False}
                lease.advance()
                self.epoch += 1
                self.snapshot_raw = None
                self.buttons = {}
                self.textareas = {}
                try:
                    result = await self.app_launcher(bundle)
                except Exception:
                    return {'ok':False, 'code':'launch_failed', 'input_emitted':None, 'completed':False}
                result.update(input_emitted=None, completed=False)
                return result
            if (tool == 'desktop' and action == 'activate_application' and set(arguments) == {'app'}
                    and valid_bundle(bundle) and (bundle in self.allowed_apps or '*' in self.allowed_apps)):
                if self.mode != 'normal':
                    return {'ok': False, 'code': self.mode, 'input_emitted': False}
                if not self._audit(action, 'allow', 'requested'):
                    return {'ok': False, 'code': 'audit_unavailable', 'input_emitted': False}
                lease.advance()
                self.epoch += 1
                self.snapshot_raw = None
                self.buttons = {}
                self.textareas = {}
                try:
                    result = await self.backend.call('activate_app', {'app': 'bundle:' + bundle})
                except Exception:
                    return {'ok': False, 'code': 'upstream_error', 'input_emitted': None, 'completed': False}
                if not result.get('ok'):
                    return {'ok': False, 'code': 'upstream_error', 'input_emitted': None, 'completed': False}
                result.update(input_emitted=None, completed=False, hint='Inspect the app to verify it became frontmost.')
                return result
            inspection = (tool == 'computer' and action == 'inspect' and 'app' in arguments and set(arguments) <= {'app', 'window', 'query'}
                          and valid_bundle(bundle) and (bundle in self.allowed_apps or '*' in self.allowed_apps)
                          and ('window' not in arguments or (type(arguments['window']) is int and arguments['window'] >= 0))
                          and ('query' not in arguments or (isinstance(arguments['query'], str) and 0 < len(arguments['query']) <= 120 and arguments['query'].isprintable())))
            if inventory or inspection:
                if not self._audit(action, 'allow', 'requested'):
                    return {'ok': False, 'code': 'audit_unavailable', 'input_emitted': False}
                if inventory:
                    try:
                        result = await self.backend.call('list_apps', {})
                        if not result.get('ok') or not isinstance(result.get('content'), str):
                            return {'ok': False, 'code': 'upstream_error', 'input_emitted': False}
                        try:
                            items = json.loads(result['content'])
                            if not isinstance(items, list) or len(items) > 10000:
                                raise ValueError('invalid inventory')
                            records = []
                            for item in items:
                                if (not isinstance(item, dict) or set(item) != {'bundle_id', 'pid'}
                                        or not isinstance(item['bundle_id'], str)
                                        or type(item['pid']) is not int or item['pid'] <= 0):
                                    raise ValueError('invalid inventory record')
                                if valid_bundle(item['bundle_id']) and (item['bundle_id'] in self.allowed_apps or '*' in self.allowed_apps):
                                    records.append(f"{item['bundle_id']} pid={item['pid']}")
                        except (ValueError, TypeError):
                            return {'ok': False, 'code': 'invalid_upstream_response', 'input_emitted': False}
                        return {'ok': True, 'content': '\n'.join(records)}
                    except Exception:
                        return {'ok': False, 'code': 'upstream_error', 'input_emitted': False}
                self.epoch += 1
                self.buttons = {}
                self.textareas = {}
                self.snapshot_generation = lease.generation()
                backend_args = {'app': 'bundle:' + bundle, 'max_elements': 150, 'max_depth': 12, 'ocr': 'never'}
                if 'window' in arguments: backend_args['window'] = arguments['window']
                if 'query' in arguments:
                    # Query filtering must not hide an explicitly secure field.
                    try:
                        preflight = await self.backend.call('get_app_state', dict(backend_args))
                    except Exception:
                        return {'ok': False, 'code': 'upstream_error', 'input_emitted': False}
                    raw = preflight.get('content')
                    if (not preflight.get('ok') or not isinstance(raw, str)
                            or not re.match(r'^[^\n]*\[' + re.escape(bundle) + r'\]\s+pid=\d+\b', raw)):
                        return {'ok': False, 'code': 'upstream_error', 'input_emitted': False}
                    if contains_secure_marker(raw):
                        self.snapshot_raw = None
                        return {'ok': True, 'content': '[redacted: secure field detected]', 'redaction_reason':redaction_reason(raw)}
                    backend_args['query'] = arguments['query']
                try:
                    result = await self.backend.call('get_app_state', backend_args)
                except Exception:
                    return {'ok': False, 'code': 'upstream_error', 'input_emitted': False}
                if not result.get('ok'):
                    code = 'unsafe_or_incomplete_tree' if result.get('code') == 'unsafe_or_incomplete_tree' else 'upstream_error'
                    return {'ok': False, 'code': code, 'input_emitted': False}
                content = result.get('content')
                if result.get('ok') and (not isinstance(content, str) or not re.match(r'^[^\n]*\[' + re.escape(bundle) + r'\]\s+pid=\d+\b', content)):
                    return {'ok': False, 'code': 'app_identity_mismatch', 'input_emitted': False}
                self.snapshot_raw = content if result.get('ok') else None
                self.snapshot_app = bundle
                self.snapshot_args = backend_args
                self.contact_refs = whatsapp_contact_refs(content) if bundle == 'net.whatsapp.WhatsApp' and 'query' not in arguments else set()
                if isinstance(result.get('content'), str):
                    result['content'] = redact_secure_fields(result['content'])
                    if result['content'] == '[redacted: secure field detected]':
                        result['redaction_reason'] = redaction_reason(content)
                        if result['redaction_reason'] == 'unverified_field_content':
                            result['redaction_details'] = unverified_field_diagnostic(content)
                        self.snapshot_raw = None
                    if result.get('ok'):
                        for line in result['content'].split('\n'):
                            match = _BUTTON_LINE.match(line)
                            if match and (bundle != 'com.apple.calculator' or match.group(2) in _SAFE_BUTTONS):
                                self.buttons[match.group(1)] = match.group(2)
                    if result.get('ok'):
                        for line in result['content'].split('\n'):
                            match = _AREA_LINE.match(line)
                            if match:
                                self.textareas[match.group(1)] = 'AX' + match.group(2)
                return result
            if (tool == 'desktop' and action == 'visual_click' and set(arguments) == {'x','y'}
                    and all(type(arguments[k]) in (int, float) and -100000 <= arguments[k] <= 100000 for k in ('x','y'))
                    and self.snapshot_app is not None and self.snapshot_raw is not None
                    and await self._complete_visual_preflight(self.snapshot_raw, self.snapshot_app, self.snapshot_args)):
                if not self._audit(action, 'confirm', 'confirmation_required'):
                    return {'ok':False, 'code':'audit_unavailable', 'input_emitted':False}
                x, y = arguments['x'], arguments['y']
                token = self.confirmations.issue({'epoch':self.epoch, 'kind':'visual_click', 'x':x, 'y':y})
                return {'ok':False, 'code':'confirmation_required', 'input_emitted':False, 'token':token,
                        'summary':f'Click screen position ({x}, {y}) in {self.snapshot_app}. Ask the user before confirming.'}
            if tool == 'computer' and action == 'click' and set(arguments) == {'ref'} and isinstance(arguments['ref'], str):
                ref = arguments['ref']
                if ref in self.buttons:
                    name = self.buttons[ref]
                    if self.snapshot_app == 'net.whatsapp.WhatsApp' and (ref in self.contact_refs or name.strip('\u200e\u200f ') == 'New Chat' or re.fullmatch(r'\+?[0-9][0-9 ]{4,22} \(You\), Message yourself', ''.join(c for c in name if unicodedata.category(c) != 'Cf'))):
                        return await self._execute_input({'epoch': self.epoch, 'ref': ref, 'name': name, 'kind': 'click'}, lease, confirmed=False)
                    if not self._audit(action, 'confirm', 'confirmation_required'):
                        return {'ok': False, 'code': 'audit_unavailable', 'input_emitted': False}
                    name = self.buttons[ref]
                    token = self.confirmations.issue({'epoch': self.epoch, 'ref': ref, 'name': name, 'kind': 'click'})
                    return {'ok': False, 'code': 'confirmation_required', 'input_emitted': False, 'token': token,
                            'summary': f'Click button {name!r} in {self.snapshot_app}. Ask the user before confirming.'}
            if tool == 'computer' and action == 'type' and set(arguments) in ({'ref', 'text'}, {'ref', 'text', 'mode'}):
                ref, value = arguments['ref'], arguments['text']
                mode = arguments.get('mode', 'insert')
                if (isinstance(ref, str) and ref in self.textareas and isinstance(value, str)
                        and mode in {'insert', 'replace'} and 0 < len(value) <= (2000 if mode == 'replace' else 200)
                        and all(c.isprintable() or (self.snapshot_app == 'com.apple.TextEdit' and c in '\n\t') for c in value)):
                    if not self._audit(action, 'confirm', 'confirmation_required'):
                        return {'ok': False, 'code': 'audit_unavailable', 'input_emitted': False}
                    token = self.confirmations.issue({'epoch': self.epoch, 'ref': ref, 'text': value, 'kind': 'type', 'mode': mode, 'role':self.textareas[ref]})
                    return {'ok': False, 'code': 'confirmation_required', 'input_emitted': False, 'token': token,
                            'summary': (f'Type {value!r} into {self.snapshot_app}. Ask the user before confirming.' if mode == 'insert' else
                                        f'Replace the field in {self.snapshot_app} with {value!r}. Ask the user before confirming.')}
            if (tool == 'computer' and action == 'press_key' and valid_key_request(arguments)
                    and self.snapshot_app is not None and self.snapshot_raw is not None
                    and self.snapshot_args is not None and 'query' not in self.snapshot_args):
                if not self._audit(action, 'confirm', 'confirmation_required'):
                    return {'ok': False, 'code': 'audit_unavailable', 'input_emitted': False}
                key, mods = arguments['key'], list(arguments['modifiers'])
                token = self.confirmations.issue({'epoch': self.epoch, 'kind': 'press_key', 'key':key, 'modifiers':mods})
                return {'ok': False, 'code': 'confirmation_required', 'input_emitted': False, 'token': token,
                        'summary': f'Press {"+".join(mods + [key])} in {self.snapshot_app}. Ask the user before confirming.'}
            if (tool == 'computer' and action == 'scroll' and set(arguments) == {'direction','amount'}
                    and isinstance(arguments.get('direction'), str) and arguments['direction'] in {'up','down','left','right'}
                    and type(arguments.get('amount')) is int and 1 <= arguments['amount'] <= 100
                    and self.snapshot_app is not None and self.snapshot_raw is not None
                    and self.snapshot_args is not None and 'query' not in self.snapshot_args):
                if not self._audit(action, 'confirm', 'confirmation_required'):
                    return {'ok': False, 'code': 'audit_unavailable', 'input_emitted': False}
                direction, amount = arguments['direction'], arguments['amount']
                token = self.confirmations.issue({'epoch':self.epoch, 'kind':'scroll', 'direction':direction, 'amount':amount})
                return {'ok':False, 'code':'confirmation_required', 'input_emitted':False, 'token':token,
                        'summary':f'Scroll {direction} {amount} lines in {self.snapshot_app}. Ask the user before confirming.'}
            if tool == 'computer' and action == 'confirm' and set(arguments) == {'token'} and isinstance(arguments['token'], str):
                approved = self.confirmations.consume(arguments['token'])
                if approved is None:
                    return {'ok': False, 'code': 'invalid_confirmation', 'input_emitted': False}
                return await self._execute_input(approved, lease)
            if not self._audit(action, 'deny', 'not_implemented'):
                return {'ok': False, 'code': 'audit_unavailable', 'input_emitted': False}
            return {'ok': False, 'code': 'not_implemented', 'input_emitted': False,
                    'message': 'Operation disabled until target safety checks and desktop verification are implemented.'}

    async def _execute_input(self, approved, lease, confirmed=True):
        if (approved['epoch'] != self.epoch or self.snapshot_generation != lease.generation()
                or (approved['kind'] == 'click' and self.buttons.get(approved['ref']) != approved['name'])
                or (approved['kind'] == 'type' and self.textareas.get(approved['ref']) != approved['role'])):
            return {'ok': False, 'code': 'stale_ref', 'input_emitted': False}
        # Re-read immediately before input. The child resets refs on each
        # inspection, so the snapshot must still match apart from foreground state.
        try:
            fresh = await self.backend.call('get_app_state', dict(self.snapshot_args))
        except Exception:
            return {'ok': False, 'code': 'upstream_error', 'input_emitted': False}
        if not fresh.get('ok') or comparable_snapshot(fresh.get('content')) != comparable_snapshot(self.snapshot_raw):
            self.epoch += 1
            self.buttons = {}
            self.textareas = {}
            return {'ok': False, 'code': 'stale_ref', 'input_emitted': False}
        if approved['kind'] == 'visual_click' and not await self._complete_visual_preflight(fresh.get('content'), self.snapshot_app, self.snapshot_args):
            return {'ok':False, 'code':'unsafe_or_incomplete_tree', 'input_emitted':False}
        if not self._audit(approved['kind'], 'confirmed' if confirmed else 'allow', 'requested'):
            return {'ok': False, 'code': 'audit_unavailable', 'input_emitted': False}
        lease.advance()
        self.epoch += 1
        self.buttons = {}
        self.textareas = {}
        try:
            if approved['kind'] == 'click':
                result = await self.backend.call('click', {'element_id': approved['ref'], 'return_state': True, 'handsmcp_expected_name': approved['name'], 'handsmcp_expected_bundle': self.snapshot_app})
            elif approved['kind'] == 'visual_click':
                result = await self.backend.call('click', {'app':'bundle:' + self.snapshot_app, 'x':approved['x'], 'y':approved['y']})
            elif approved['kind'] == 'press_key':
                result = await self.backend.call('press_key', {'app':'bundle:' + self.snapshot_app, 'key':approved['key'], 'modifiers':approved['modifiers']})
            elif approved['kind'] == 'scroll':
                result = await self.backend.call('scroll', {'app':'bundle:' + self.snapshot_app, 'direction':approved['direction'], 'amount':approved['amount']})
            elif approved['mode'] == 'replace':
                args = {'element_id': approved['ref'], 'value': approved['text'], 'return_state': True, 'handsmcp_expected_bundle': self.snapshot_app}
                if approved['role'] != 'AXTextArea': args['handsmcp_expected_role'] = approved['role']
                result = await self.backend.call('set_value', args)
            else:
                args = {'element_id': approved['ref'], 'text': approved['text'], 'return_state': True, 'handsmcp_expected_bundle': self.snapshot_app}
                if approved['role'] != 'AXTextArea': args['handsmcp_expected_role'] = approved['role']
                result = await self.backend.call('type_text', args)
        except Exception:
            return {'ok': False, 'code': 'upstream_error', 'input_emitted': None, 'completed': False, 'hint': 'Inspect state before retrying.'}
        if not result.get('ok'):
            if approved['kind'] == 'visual_click':
                reasons = {
                    'error: coordinate is outside the selected app window; no input was sent': ('coordinate_outside_window', False),
                    'error: accessibility press failed; inspect state before retrying': ('accessibility_press_failed', None),
                }
                reason = reasons.get(result.get('content')) if isinstance(result.get('content'), str) else None
                if reason:
                    return {'ok':False, 'code':reason[0], 'input_emitted':reason[1], 'completed':False,
                            'hint':'Inspect state before retrying.'}
            if (approved['kind'] == 'press_key' and isinstance(result.get('content'), str)
                    and result['content'].startswith('error: ')
                    and result['content'].endswith(' is disabled or cannot be verified; no menu action was sent')):
                return {'ok':False, 'code':'shortcut_unavailable', 'input_emitted':False, 'completed':False,
                        'hint':'The menu item is disabled or its state could not be verified. Inspect the app.'}
            code = replacement_error_code(result.get('content')) if approved['kind'] == 'type' and approved.get('mode') == 'replace' else 'upstream_error'
            return {'ok': False, 'code': code, 'input_emitted': None, 'completed': False, 'hint': 'Inspect state before retrying.'}
        if isinstance(result.get('content'), str):
            result['content'] = redact_secure_fields(result['content'])
        result['input_emitted'] = None
        result['completed'] = False
        result['hint'] = 'Verify the resulting state; input delivery alone does not prove completion.'
        return result
