# Security and current limits

This prototype can control the local desktop once permission is granted. It now accepts requests for named button clicks and text-area typing across permitted apps, subject to confirmation and native identity checks. Other input is refused. The generalized input paths have not yet been verified live. Clients must show the summary to the human and wait for explicit approval before sending the token. The token mechanism cannot prove that the human was asked, so keep the agent's own approval prompts enabled.

Never use this prototype to enter passwords, verification codes or other secrets. Secure-field output suppression uses known markers in unstructured upstream text, and unknown representations may not be detected. General app access, password-manager denial, OCR, screenshots and other security requirements are not complete. Do not treat this software as a complete security boundary.

Create `~/.handsmcp/STOP` to block calls. Delete that file to resume. `policy.yaml` supports `read_only` and `dry_run`, which block input, and configures the per-process rate limit and confirmation expiry. Restart the server after changing it. The audit log `~/.handsmcp/logs/audit.jsonl` stores only a small allowlist of request metadata, not typed text. It is not yet a full action/result audit.

Separate handsmcp processes cooperate via a file lock and a generation counter. Other apps and automation tools do not participate. A fresh text snapshot is checked before input, but external changes can still race between the check and action. Inspect before retrying after any uncertain result.
