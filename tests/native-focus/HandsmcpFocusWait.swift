/// Poll an observation only. No input is sent during retries.
func waitForVerifiedFocus(attempts: Int, delay: () -> Void, verify: () -> Bool) -> Bool {
    guard attempts > 0 else { return false }
    for attempt in 0..<attempts {
        if verify() { return true }
        if attempt + 1 < attempts { delay() }
    }
    return false
}
