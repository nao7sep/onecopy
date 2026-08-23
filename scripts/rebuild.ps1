Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$scriptExitCode = 0

# rebuild: produce a fresh release build and launch it. Slow — run this after
# changing source. tauri build runs the frontend's production type check and
# bundle (via beforeBuildCommand) and compiles the Rust release binary, so type,
# CSP, and packaged-layout errors that run-dev hides surface here. On Windows the
# launchable artifact is the release .exe itself, so --no-bundle skips the nsis
# installer. run-built is the fast, no-build launcher after this.

function Set-Utf8Console {
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [Console]::InputEncoding = $utf8NoBom
    [Console]::OutputEncoding = $utf8NoBom
    $global:OutputEncoding = $utf8NoBom
    if (Get-Command chcp.com -ErrorAction SilentlyContinue) {
        & chcp.com 65001 > $null
        $null = $LASTEXITCODE
    }
}

function Write-Step {
    param([string]$Message)
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Require-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Missing required command: $Name"
    }
}

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [int[]]$AllowedExitCodes = @(0)
    )

    & $FilePath @ArgumentList
    $exitCode = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
    if ($AllowedExitCodes -notcontains $exitCode) {
        throw "Command failed with exit code ${exitCode}: $FilePath $($ArgumentList -join ' ')"
    }
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoDir = Split-Path -Parent $scriptDir
$crate = "onecopy"
$exePath = Join-Path $repoDir "src-tauri/target/release/$crate.exe"

try {
    Set-Utf8Console
    Require-Command node
    Require-Command npm
    Require-Command cargo
    Require-Command rustc

    Set-Location $repoDir

    Write-Step "Installing dependencies"
    Invoke-Native -FilePath "npm" -ArgumentList @("install")

    # Remove stale frontend output so a build that fails to emit a file can't be
    # masked by a leftover artifact from a previous run. Only the frontend output
    # is cleaned: cargo is incremental, and cleaning src-tauri/target would force
    # a multi-minute recompile of the Rust binary.
    Write-Step "Cleaning previous frontend build"
    if (Test-Path "dist") { Remove-Item -Recurse -Force "dist" }

    # tauri build runs beforeBuildCommand (the frontend `tsc && vite build`) and
    # compiles the Rust release binary; --no-bundle skips the nsis installer — the
    # launchable artifact is the release .exe.
    Write-Step "Building release binary"
    Invoke-Native -FilePath "node_modules/.bin/tauri.cmd" -ArgumentList @("build", "--no-bundle")

    if (-not (Test-Path $exePath)) {
        throw "Build did not produce $crate.exe under src-tauri/target/release/."
    }

    # GUI app: launch non-blocking via Start-Process.
    Write-Step "Launching the built app"
    Start-Process -FilePath $exePath
}
catch {
    Write-Host ""
    Write-Host "onecopy rebuild failed: $($_.Exception.Message)" -ForegroundColor Red
    $scriptExitCode = 1
}
finally {
    Read-Host "Press Enter to close" | Out-Null
}

exit $scriptExitCode
