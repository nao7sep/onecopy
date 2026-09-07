# Package OneCopy for Windows into artifacts/: the tauri-built NSIS setup.exe + a
# portable .zip of the self-contained release exe. `tauri build` produces the
# setup.exe; this script runs it and collects the outputs. Output goes to
# artifacts/, NOT dist/ (dist/ is Vite's frontend build dir). Assumes node_modules
# and a Rust toolchain are present (the workflow installs them).
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-NativeCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string[]]$ArgumentList
    )

    $output = & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath failed with exit code $LASTEXITCODE"
    }

    return $output
}

$Repo = Split-Path -Parent $PSScriptRoot
Set-Location $Repo

$AppName = "OneCopy"
$TauriCli = Join-Path $Repo "node_modules/.bin/tauri.cmd"
$ArtifactDirectory = Join-Path $Repo "artifacts"
$NsisDirectory = Join-Path $Repo "src-tauri/target/release/bundle/nsis"
$PortableExecutable = Join-Path $Repo "src-tauri/target/release/onecopy.exe"

# Packaging may only collect outputs produced by this invocation. Keep Cargo's
# incremental cache, but remove every release artifact that could make a failed
# native command look successful.
Remove-Item -Recurse -Force $ArtifactDirectory -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force $NsisDirectory -ErrorAction SilentlyContinue
Remove-Item -Force $PortableExecutable -ErrorAction SilentlyContinue

if (-not (Test-Path -PathType Leaf $TauriCli)) {
    throw "Missing local Tauri CLI. Run npm install before packaging."
}

$VersionOutput = @(Invoke-NativeCommand -FilePath "node" -ArgumentList @(
    "-p",
    "require('./src-tauri/tauri.conf.json').version"
))
$Versions = @($VersionOutput | ForEach-Object { $_.ToString().Trim() } | Where-Object { $_ })
if ($Versions.Count -ne 1) {
    throw "Expected one non-empty version from src-tauri/tauri.conf.json; received $($Versions.Count)"
}
$Version = $Versions[0]

New-Item -ItemType Directory -Force -Path $ArtifactDirectory | Out-Null

# Builds the frontend, the Rust release binary, and the NSIS setup.exe.
Invoke-NativeCommand -FilePath $TauriCli -ArgumentList @("build", "--bundles", "nsis")

$Setups = @(Get-ChildItem -Path $NsisDirectory -Filter "*-setup.exe" -File -ErrorAction SilentlyContinue)
if ($Setups.Count -ne 1) {
    throw "Expected one NSIS setup.exe from this build; found $($Setups.Count)"
}
Copy-Item $Setups[0].FullName "artifacts/$AppName-$Version-setup.exe"

# Portable: the self-contained release exe. Tauri embeds the frontend into the
# binary; WebView2 is a system runtime present on Windows 10/11. The exe is named
# after the Cargo crate (onecopy), not the productName.
if (-not (Test-Path -PathType Leaf $PortableExecutable)) {
    throw "tauri build did not produce the portable onecopy.exe"
}
Compress-Archive -Path "src-tauri/target/release/onecopy.exe", "LICENSE", "THIRD_PARTY_NOTICES" -DestinationPath "artifacts/$AppName-$Version-win.zip" -Force

Get-ChildItem artifacts
