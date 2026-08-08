#!/usr/bin/env bash
set -euo pipefail

# run-dev: run the app from source with live reload, in its loosest configuration.
# For active coding and debugging. The strict, production-faithful launchers are
# run-built (launch the existing packaged app bundle without rebuilding) and
# rebuild (build and package a fresh bundle, then launch).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

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
  if [[ "$status" -ne 0 && "$status" -ne 130 ]]; then
    echo
    echo "onecopy run-dev failed with exit code $status."
    read -r -p "Press Enter to close..."
  fi
}

stop_port() {
  local port="$1"
  local pids
  pids="$(lsof -ti tcp:"$port" 2>/dev/null || true)"
  if [[ -n "$pids" ]]; then
    echo "Stopping processes on port $port: $pids"
    kill $pids 2>/dev/null || true
    sleep 1
    pids="$(lsof -ti tcp:"$port" 2>/dev/null || true)"
    if [[ -n "$pids" ]]; then
      kill -9 $pids 2>/dev/null || true
    fi
  fi
}

trap 'pause_on_failure $?' EXIT

require_command node
require_command npm
require_command cargo
require_command rustc
require_command lsof

cd "$REPO_DIR"

log_step "Stopping stale development listeners"
# 1721 is this app's Vite dev port (fleet-unique so this port-kill never
# cross-fires into dropkick's 1521 or quickdeck's 1621).
stop_port 1721

log_step "Installing dependencies required for launch"
npm install

log_step "Starting OneCopy in development mode"
npm run tauri dev
