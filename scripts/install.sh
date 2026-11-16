#!/bin/bash
# Build the pinned Munim runtime and install handsmcp in a local venv.
set -euo pipefail
if [[ ${1:-} == --help ]]; then
  printf '%s\n' 'Usage: ./scripts/install.sh' 'Builds the pinned macOS runtime and installs the Python package in .venv.' 'Existing runtime and virtual environment are preserved.'
  exit 0
fi
if [[ $# -ne 0 ]]; then printf '%s\n' 'Unknown argument; use --help.' >&2; exit 2; fi
root=$(cd "$(dirname "$0")/.." && pwd)
if [[ $(uname -s) != Darwin ]]; then printf '%s\n' 'handsmcp currently requires macOS.' >&2; exit 1; fi
for cmd in git swift python3; do command -v "$cmd" >/dev/null || { printf 'Missing required tool: %s\n' "$cmd" >&2; exit 1; }; done
python3 -c 'import sys; assert sys.version_info >= (3, 11), "Python 3.11 or newer is required"'
if [[ ! -e "$root/runtime/munim-computer-use" ]]; then
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/handsmcp-build.XXXXXXXX")
  trap 'rm -rf "$tmp"' EXIT
  git clone --quiet https://github.com/munimtechnologies/munim-computer-use.git "$tmp/munim"
  git -C "$tmp/munim" checkout --quiet --detach 883a4e181409f203b4b51f8219d88e0af43b1034
  for patch in munim-secure-subrole munim-verified-fields munim-menu-status munim-bridge-optout munim-visual-click munim-coordinate-hit-test munim-chrome-fields munim-set-value-focus munim-focus-wait; do
    git -C "$tmp/munim" apply "$root/patches/$patch.patch"
  done
  (cd "$tmp/munim/macos" && swift build -c release)
  mkdir -p "$root/runtime"
  # Do not replace an install that appeared while compiling.
  if [[ -e "$root/runtime/munim-computer-use" ]]; then printf '%s\n' 'Runtime appeared during build; leaving it unchanged.' >&2; exit 1; fi
  cp "$tmp/munim/macos/.build/release/munim-computer-use" "$root/runtime/munim-computer-use"
else
  printf '%s\n' 'Existing runtime retained.'
fi
if [[ ! -e "$root/.venv" ]]; then python3 -m venv "$root/.venv"; fi
if [[ ! -x "$root/.venv/bin/python" ]]; then printf '%s\n' 'Existing .venv is not usable; leaving it unchanged.' >&2; exit 1; fi
"$root/.venv/bin/python" -m pip install -e "$root"
printf '\n%s\n' 'Installed. Run .venv/bin/handsmcp install --agent antigravity --backend cua for connection instructions.'
printf '%s\n' 'The Cua backend also needs Cua Driver installed separately; see docs/INSTALL.md.'
