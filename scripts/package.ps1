# Package OneCopy for Windows into artifacts/: the tauri-built NSIS setup.exe + a
# portable .zip of the self-contained release exe. `tauri build` produces the
# setup.exe; this script runs it and collects the outputs. Output goes to
# artifacts/, NOT dist/ (dist/ is Vite's frontend build dir). Assumes node_modules
# and a Rust toolchain are present (the workflow installs them).
$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent $PSScriptRoot
Set-Location $Repo

$AppName = "OneCopy"
$Version = (node -p "require('./src-tauri/tauri.conf.json').version")

Remove-Item -Recurse -Force artifacts -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path artifacts | Out-Null

# Builds the frontend, the Rust release binary, and the NSIS setup.exe.
npx tauri build --bundles nsis

$setup = Get-ChildItem src-tauri/target/release/bundle/nsis/*-setup.exe | Select-Object -First 1
if (-not $setup) { throw "tauri build did not produce an NSIS setup.exe" }
Copy-Item $setup.FullName "artifacts/$AppName-$Version-setup.exe"

# Portable: the self-contained release exe. Tauri embeds the frontend into the
# binary; WebView2 is a system runtime present on Windows 10/11. The exe is named
# after the Cargo crate (onecopy), not the productName.
Compress-Archive -Path "src-tauri/target/release/onecopy.exe" -DestinationPath "artifacts/$AppName-$Version-win.zip" -Force

Get-ChildItem artifacts
