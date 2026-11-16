# ADR 001: Select Munim for the macOS prototype

Status: selected for the initial macOS implementation, subject to screenshot, security and agent-integration verification.

## Evidence

On this Mac, Microsoft Hybrid Computer Use (HCU), commit `697d3fbda04cb322915866bab1aad79c726a61e3`, built successfully and passed MCP initialize and tool discovery. Its live AX calls continued to report Accessibility unavailable after permission changes, so the TextEdit, Calculator and browser tasks could not be run through it. Its GUI screenshot call also reported Screen Recording denied. HCU is MIT licensed with third-party notices; its Go server uses a Swift macOS provider and exposes two MCP tools.

Munim Computer Use, commit `883a4e181409f203b4b51f8219d88e0af43b1034`, built on this Mac with warnings and passed MCP tool discovery (38 tools). Its macOS implementation is Swift; Windows and Linux use Rust. It is Apache-2.0 licensed. In direct MCP tests on this Mac, it listed apps, discovered and typed `handsmcp comparison test` into an unsaved TextEdit document and read the resulting text; used accessibility button refs to calculate 2 + 3 and read 5; and filled a local Chrome form and read the expected output. The first Calculator run produced 60003 because Clear preserved an earlier expression; using All Clear and repeating produced 5. Individual observed action times ranged roughly 210–1071 ms; these are single-run observations, not a benchmark. Munim's TextEdit screenshot returned a Screen Recording permission error, even after the user enabled entries. Screenshots are unverified.

## Decision and limits

Choose Munim for the initial macOS implementation because it completed the semantic native and browser checks here while HCU could not pass its permission gate. This is not proof that Munim is universally better or secure. HCU remains a viable alternative once its permission identity is fixed. Neither engine has been compared on a full benchmark, OCR, stale refs, security, idle CPU or memory; Windows and Linux are untested. The prototype exposes only application listing and refuses all input until its own policy enforcement is implemented. The direct Munim test calls were not mediated by that prototype.

The brief's `munim-ai/munim` and `open-computer-use/open-computer-use` URLs returned 404; `a11y-computer-use/a11y-computer-use` could not be verified. The separately identified iFurySt/open-codex-computer-use redirects to iFurySt/open-computer-use and has not been tested.
