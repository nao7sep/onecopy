#!/usr/bin/env bash
set -euo pipefail

# Package OneCopy for macOS into artifacts/: the tauri-built .dmg installer + a
# portable .zip of the .app. `tauri build` produces the .app and the .dmg; this
# script runs it and collects/renames the outputs. Output goes to artifacts/, NOT
# dist/ — dist/ is Vite's frontend build dir (tauri build regenerates it), so the
# two must not share a folder. Assumes node_modules and a Rust toolchain are
# present (the workflow installs them; locally, run `npm install` first). Per the
# app-release-conventions, packaging lives here so CI stays minimal.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO"

APP_NAME="OneCopy"
VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
TAURI_CLI="node_modules/.bin/tauri"
DMG_DIR="src-tauri/target/release/bundle/dmg"

cleanup_dmg_scratch() {
  if [[ -d "$DMG_DIR" ]]; then
    find "$DMG_DIR" -maxdepth 1 -type f -name 'rw.*.dmg' -delete
  fi
}

# create-dmg uses rw.*.dmg as bounded scratch space. Remove leftovers from an
# interrupted prior package, and clean the same scratch files on every exit.
cleanup_dmg_scratch
trap cleanup_dmg_scratch EXIT

if [[ ! -x "$TAURI_CLI" ]]; then
  echo "Missing local Tauri CLI. Run npm install before packaging." >&2
  exit 1
fi

rm -rf artifacts
mkdir -p artifacts

# Builds the frontend (beforeBuildCommand), the Rust release binary, the .app, and
# the .dmg. --bundles overrides tauri.conf.json's targets so macOS emits app + dmg.
"$TAURI_CLI" build --bundles app,dmg

DMG="$(ls "$DMG_DIR"/*.dmg | head -1)"
APP="$(ls -d src-tauri/target/release/bundle/macos/*.app | head -1)"
[ -f "$DMG" ] && [ -d "$APP" ] || { echo "tauri build did not produce the expected .dmg/.app" >&2; exit 1; }

cp "$DMG" "artifacts/$APP_NAME-$VERSION.dmg"
# Portable: a zip of the .app without AppleDouble resource-fork sidecars.
ditto -c -k --norsrc --keepParent "$APP" "artifacts/$APP_NAME-$VERSION-mac.zip"

echo "macOS artifacts in artifacts/:"
ls -la artifacts/
