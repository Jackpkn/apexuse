# Initial macOS comparison

Exploratory single runs, not a statistically useful benchmark. The direct tests called each upstream MCP server; they did not pass through handsmcp. Run on the same Mac and user session. The HCU and Munim versions are recorded in ADR-001.

| Check | HCU | Munim |
| --- | --- | --- |
| macOS build | Passed | Passed, with compiler/linker warnings |
| MCP handshake | Passed, AX and GUI | Passed, 38 tools |
| Live accessibility | Permission denied | App inventory, TextEdit, Calculator and local Chrome form accessible |
| TextEdit | Not run | Typed a phrase and observed it in the text area |
| Calculator | Not run | Observed 2 + 3 = 5 after All Clear; first trial failed due to existing expression |
| Chrome form | Not run | Entered text, checked a box, clicked the local page button; observed expected output |
| Screenshot of TextEdit | Permission denied | Permission denied |

Examples of one-call elapsed times with returned state: TextEdit input 367 ms; Calculator clicks 394–583 ms after reset; local Chrome form actions 210–1071 ms. These include waits in the tools, are unrepeatable single samples and do not support a general speed comparison. HCU could not complete comparable actions, so there is no head-to-head success-rate or latency result. Token use, CPU, memory, fault recovery, OCR and security are unmeasured. Full 3-mode and multi-run benchmarking remains outstanding.
