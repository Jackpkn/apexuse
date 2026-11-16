"""In-process sliding-window request limiter."""
from collections import deque
from time import monotonic

class RateLimiter:
    def __init__(self, limit=120, window_seconds=60, clock=monotonic):
        if limit < 1 or window_seconds <= 0:
            raise ValueError('invalid rate limit')
        self.limit, self.window_seconds, self.clock = limit, window_seconds, clock
        self.events = deque()

    def allow(self):
        now = self.clock()
        while self.events and self.events[0] <= now - self.window_seconds:
            self.events.popleft()
        if len(self.events) >= self.limit:
            return False
        self.events.append(now)
        return True
