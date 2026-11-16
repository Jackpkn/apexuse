"""Validate the supported fail-closed runtime settings."""
from dataclasses import dataclass
from pathlib import Path
import re
import yaml

DEFAULT_APPS = ('com.apple.TextEdit', 'com.apple.calculator')
_DENIED = ('password','1password','onepassword','bitwarden','keychain','dashlane','lastpass','nordpass','proton.pass','keepass','bank','finance','wallet')

def is_protected(value):
    return isinstance(value, str) and any(x in value.casefold() for x in _DENIED)
_BUNDLE = re.compile(r'^[A-Za-z0-9][A-Za-z0-9_-]*(?:\.[A-Za-z0-9][A-Za-z0-9_-]*)+$')

def valid_bundle(app):
    return isinstance(app, str) and bool(_BUNDLE.fullmatch(app)) and not is_protected(app)

def validate_apps(apps):
    if not isinstance(apps, (list, tuple)) or len(apps) > 100:
        raise ValueError('allowed_apps must be a list of bundle identifiers')
    for app in apps:
        if not isinstance(app, str) or (app != '*' and (not _BUNDLE.fullmatch(app) or is_protected(app))):
            raise ValueError('invalid or protected app bundle identifier')
    return tuple(apps)

@dataclass(frozen=True)
class Settings:
    mode: str = 'normal'
    confirmation: str = 'direct'
    require_confirmation: bool = False
    rate_limit_per_minute: int = 120
    confirmation_ttl_seconds: int = 120
    allowed_apps: tuple[str, ...] = DEFAULT_APPS

def load_settings(path):
    values = yaml.safe_load(Path(path).read_text())
    if not isinstance(values, dict):
        raise ValueError('policy must be a mapping')
    unknown = set(values) - {'mode','confirmation','require_confirmation','rate_limit_per_minute','confirmation_ttl_seconds','allowed_apps'}
    if unknown: raise ValueError('unknown policy fields')
    if 'confirmation' in values:
        c = values['confirmation']
        if not isinstance(c, str) or c.casefold() not in {'direct','none','required','always'}:
            raise ValueError('invalid confirmation mode')
        values['confirmation'] = c.casefold()
        values.setdefault('require_confirmation', values['confirmation'] in {'required','always'})
    if 'require_confirmation' in values and not isinstance(values['require_confirmation'], bool):
        raise ValueError('invalid require_confirmation')
    if 'allowed_apps' in values: values['allowed_apps'] = validate_apps(values['allowed_apps'])
    settings = Settings(**values)
    if not isinstance(settings.mode, str) or settings.mode not in {'normal','read_only','dry_run'}:
        raise ValueError('invalid policy mode')
    for value in (settings.rate_limit_per_minute, settings.confirmation_ttl_seconds):
        if type(value) is not int or not 1 <= value <= 3600: raise ValueError('invalid policy limit')
    return settings
