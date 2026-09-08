#!/usr/bin/env bash
set -euo pipefail

# run-dev: run the app from source with live reload, in its loosest configuration.
# For active coding and debugging. The strict, production-faithful launchers are
# run-built (launch the existing packaged app bundle without rebuilding) and
# rebuild (build and package a fresh bundle, then launch).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
RUNTIME_HELPER="$SCRIPT_DIR/launcher-runtime.mjs"
RUNTIME_TOKEN="run-dev-$$-$(date +%s)-$RANDOM"
DEV_HOST="${TAURI_DEV_HOST:-127.0.0.1}"
DEV_URL="http://$DEV_HOST:28867"

log_step() {
  printf '\n==> %s\n' "$1"
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

pause_on_failure() {
  local status="$1"
  if [[ "$status" -ne 0 && ( "$status" -lt 128 || "$status" -gt 143 ) ]]; then
    echo
    echo "onecopy run-dev failed with exit code $status."
    read -r -p "Press Enter to close..."
  fi
}

cleanup() {
  local status="$?"
  trap - EXIT
  if node "$RUNTIME_HELPER" is-owner "$RUNTIME_TOKEN" >/dev/null 2>&1; then
    node "$RUNTIME_HELPER" stop-if-owner "$RUNTIME_TOKEN" tauri "OneCopy" "onecopy" >/dev/null 2>&1 || true
  else
    status=0
  fi
  pause_on_failure "$status"
  exit "$status"
}

trap cleanup EXIT

require_command node
require_command npm
require_command cargo
require_command rustc

cd "$REPO_DIR"

log_step "Replacing any existing OneCopy runtime"
node "$RUNTIME_HELPER" claim "$RUNTIME_TOKEN"
node "$RUNTIME_HELPER" stop tauri "OneCopy" "onecopy"
node "$RUNTIME_HELPER" check-endpoint "$DEV_HOST" 28867
if [[ -n "${TAURI_DEV_HOST:-}" ]]; then
  node "$RUNTIME_HELPER" check-endpoint "$DEV_HOST" 29587
fi

log_step "Installing dependencies required for launch"
npm install --no-audit --no-fund

log_step "Starting OneCopy in development mode"
npm run tauri dev &
DEV_PID=$!
node "$RUNTIME_HELPER" wait-http "$DEV_URL" 60000
node "$RUNTIME_HELPER" wait-process "$REPO_DIR/src-tauri/target/debug/onecopy" 180000
log_step "OneCopy is ready at $DEV_URL"
wait "$DEV_PID"
