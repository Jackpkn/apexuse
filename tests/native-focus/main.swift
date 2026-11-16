var calls = 0
let ready = waitForVerifiedFocus(attempts: 4, delay: {}) { calls += 1; return calls == 3 }
precondition(ready && calls == 3)
calls = 0
let refused = waitForVerifiedFocus(attempts: 3, delay: {}) { calls += 1; return false }
precondition(!refused && calls == 3)
print("focus wait checks passed")
