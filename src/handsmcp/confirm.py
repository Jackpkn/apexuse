"""Short-lived, single-use authorization records."""
from copy import deepcopy
import secrets
import time


class ConfirmationStore:
    def __init__(self, ttl_seconds=120, clock=time.monotonic):
        if ttl_seconds <= 0:
            raise ValueError("ttl_seconds must be positive")
        self.ttl_seconds = ttl_seconds
        self.clock = clock
        self._pending = {}

    def issue(self, action):
        now = self.clock()
        self._pending = {token: item for token, item in self._pending.items() if item[0] > now}
        token = secrets.token_urlsafe(32)
        self._pending[token] = (now + self.ttl_seconds, deepcopy(action))
        return token

    def consume(self, token):
        item = self._pending.pop(token, None)
        if item is None or self.clock() >= item[0]:
            return None
        return deepcopy(item[1])
