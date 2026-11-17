# ApexUse Troubleshooting

## 1. macOS Permissions & Host-App TCC Attribution

- **Host app owns TCC grants:** On macOS 14+, Accessibility and Screen Recording permissions for stdio MCP child processes are attributed to the responsible **host application** (`Terminal`, `iTerm2`, `Codex`, `Cursor`, `Visual Studio Code`, `Antigravity`, etc.), not `apexuse-mcp` itself.
- **Run guided onboarding:**
  ```sh
  npx -y apexuse-mcp doctor --onboard
  ```
  The native window displays the detected host app, opens System Settings directly (`Privacy & Security → Accessibility` and `Screen Recording`), and polls live status.
- **Screen Recording restart note:** When toggling Screen Recording ON in System Settings, macOS may prompt **Quit & Reopen** / **Later**. If you click *Later* (or if `Screen Recording` still shows `● Required` after toggling), fully quit and relaunch the host app so macOS activates the new Screen Recording token for that session.
- **Headless / CI check:** `apexuse-mcp doctor --check` never launches GUI windows; on macOS it exits `1` and adds `"onboard_hint"` in JSON when either grant is missing.

## 2. Linux AT-SPI2, X11 & Wayland

Check `npx -y apexuse-mcp doctor` output (`session=x11|wayland x11=... atspi=... wayland_cap=... tesseract=...`):
- **Missing Accessibility (`atspi=false`):** Install AT-SPI2 Python GObject bindings and ensure the session D-Bus/AT-SPI bus is running:
  - Debian/Ubuntu: `sudo apt-get install at-spi2-core python3-gi gir1.2-atspi-2.0`
  - Headless CI: launch under `xvfb-run` with `dbus-x11`.
- **X11 input/capture (`x11=false`):** `LinuxDriver` dynamically loads `libX11` and `libXtst` via `dlopen` (install `libx11-6`, `libxtst6`, and optionally `xdotool` under an active `DISPLAY`).
- **Wayland (`session=wayland`):** Screen capture requires `grim`, `spectacle`, or `gnome-screenshot` (`wayland_cap=true`); typing/key helpers fall back to `wtype` or `ydotool`.
- **OCR (`tesseract=false`):** Install `tesseract-ocr` for OCR text marks in `visual_inspect`.

## 3. Windows UIA & `Ctrl+V` / Paste Blocks

- **Built-in driver:** `WindowsDriver` uses Win32 COM `IUIAutomation` (`ValuePattern`/`TextPattern`), `PrintWindow`/`BitBlt`, WinRT `Windows.Media.Ocr`, and `SendInput` with no extra downloads.
- **`target_identity_unverified` on `set_value` / `select_text`:** Raw `(x, y)` coordinates and OCR-only marks are intentionally rejected. Target an `inspect_app` `ref` (`e1..eN`) or semantic `target: { query, role? }` backed by an editable UIA element (`TextPattern` is required for partial `[start, end)` `select_text` ranges).
- **Blocked `Cmd+V` / `Ctrl+V`:** Paste shortcuts return `invalid_argument` (`Unsupported key or blocked paste shortcut`) across all OSes to prevent unintentional clipboard leaks. Use `set_value` or `type` instead.

## 4. `--probe` Timeouts & Failures

- **Cold `npx` first-run download:** `probe` and `install ... --probe` default to `10000` ms (`--timeout-ms 10000`). If `npx -y apexuse-mcp` is downloading the release binary on a slow link, raise the timeout:
  ```sh
  npx -y apexuse-mcp install codex-plugin --write --npx --probe --timeout-ms 30000
  ```
  Or pre-warm the cache first with `npx -y apexuse-mcp --version`.
- **Offline CI verification:** Pass `--mock` (`apexuse-mcp probe --mock` or `install ... --probe --mock`). `--mock` only applies to the temporary probe subprocess and is never written into `.mcp.json` or agent config files.
- **Expected handshake:** `probe` checks that the server responds to `initialize` with `serverInfo.name == "apexuse-mcp"` and returns all `6` tools on `tools/list`.

## 5. Emergency Kill Switch & Protected Apps

- If all mutations fail immediately, check for and remove `~/.apexuse/STOP` (or `$APEXUSE_STATE_DIR/STOP`).
- Password managers, wallets, keychains, banking apps, and password inputs (`AXSecureTextField`, `CurrentIsPassword`, `PASSWORD_TEXT`) are always blocked or redacted regardless of policy.
