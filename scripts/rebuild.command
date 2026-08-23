#!/usr/bin/env bash
set -euo pipefail

# rebuild: produce a fresh release build, package it into a real .app bundle, and
# launch that bundle. Slow — run this after changing source. tauri build runs the
# frontend's production type check and bundle (via beforeBuildCommand), compiles
# the Rust release binary, and packages the macOS .app, so type, CSP, and
# packaged-layout errors that run-dev hides surface here; the .app carries the
# correct CFBundleName, so the dock and menu show "OneCopy" rather than the
# lowercase crate name. run-built is the fast, no-build launcher after this.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_NAME="OneCopy"
APP_BUNDLE="src-tauri/target/release/bundle/macos/$APP_NAME.app"

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
    echo "onecopy rebuild failed with exit code $status."
    read -r -p "Press Enter to close..."
  fi
}

trap 'pause_on_failure $?' EXIT

require_command node
require_command npm
require_command cargo
require_command rustc

cd "$REPO_DIR"

log_step "Installing dependencies"
npm install

# Remove stale frontend output so a build that fails to emit a file can't be
# masked by a leftover artifact from a previous run. Only the frontend output is
# cleaned: cargo is incremental, and cleaning src-tauri/target would force a
# multi-minute recompile of the Rust binary.
log_step "Cleaning previous frontend build"
rm -rf dist

# tauri build runs beforeBuildCommand (the frontend `tsc && vite build`), compiles
# the Rust release binary, and --bundles app packages the macOS .app only (no
# dmg/nsis installer). The release build is the slow first run; cargo is
# incremental afterwards.
log_step "Building release binary and app bundle"
node_modules/.bin/tauri build --bundles app

if [[ ! -d "$APP_BUNDLE/Contents/MacOS" ]]; then
  echo "Packaging did not produce $APP_NAME.app under src-tauri/target/release/bundle/macos/." >&2
  exit 1
fi

log_step "Launching the packaged app"
open "$APP_BUNDLE"
