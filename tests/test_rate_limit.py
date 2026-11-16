from handsmcp.rate_limit import RateLimiter

def test_limit_expires_after_one_minute():
    now = [0.0]
    limiter = RateLimiter(limit=2, window_seconds=60, clock=lambda: now[0])
    assert limiter.allow() and limiter.allow()
    assert not limiter.allow()
    now[0] = 60
    assert limiter.allow()
