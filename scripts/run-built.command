#!/usr/bin/env bash
set -euo pipefail

# run-built: launch the EXISTING packaged app without rebuilding, so it starts
# instantly. This is the daily-use launcher and the one that surfaces
# production-only failures (strict CSP, packaged layout) and runs under the app's
# own bundle identity. It never builds — if you changed source, run rebuild first.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_NAME="OneCopy"
APP_BUNDLE="src-tauri/target/release/bundle/macos/$APP_NAME.app"

log_step() {
  printf '\n==> %s\n' "$1"
}

pause_on_failure() {
  local status="$1"
  if [[ "$status" -ne 0 && "$status" -ne 130 ]]; then
    echo
    echo "onecopy run-built failed with exit code $status."
    read -r -p "Press Enter to close..."
  fi
}

trap 'pause_on_failure $?' EXIT

cd "$REPO_DIR"

# No build here: this launcher must start instantly. If there is no usable bundle
# yet, stop and point at rebuild rather than launching something stale or empty.
if [[ ! -d "$APP_BUNDLE/Contents/MacOS" ]]; then
  echo "No packaged app found ($APP_BUNDLE is missing) — run rebuild first."
  exit 1
fi

built_at="$(stat -f '%Sm' -t '%Y-%m-%d %H:%M:%S %Z' "$APP_BUNDLE/Contents/MacOS" 2>/dev/null || echo 'unknown')"
log_step "Launching the existing packaged app (built: $built_at)"
echo "If you changed source since then, run rebuild instead."

open "$APP_BUNDLE"
