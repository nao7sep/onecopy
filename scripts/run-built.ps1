Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$scriptExitCode = 0

# run-built: launch the EXISTING built app without rebuilding, so it starts
# instantly. This is the daily-use launcher and the one that surfaces
# production-only failures (strict CSP, packaged layout). It never builds — if you
# changed source, run rebuild first.

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

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoDir = Split-Path -Parent $scriptDir
$runtimeHelper = Join-Path $scriptDir "launcher-runtime.mjs"
$runtimeToken = [guid]::NewGuid().ToString("N")
$crate = "onecopy"
$exePath = Join-Path $repoDir "src-tauri/target/release/$crate.exe"

try {
    Set-Utf8Console
    if (-not (Get-Command "node" -ErrorAction SilentlyContinue)) {
        throw "Missing required command: node"
    }

    Set-Location $repoDir

    # No build here: this launcher must start instantly. If there is no usable
    # build yet, stop and point at rebuild rather than launching something stale
    # or empty.
    if (-not (Test-Path $exePath)) {
        throw "No build found (src-tauri/target/release/$crate.exe is missing). Run rebuild first."
    }

    $builtAt = (Get-Item $exePath).LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")
    & node $runtimeHelper claim $runtimeToken
    & node $runtimeHelper stop tauri "OneCopy" "onecopy"
    if ($LASTEXITCODE -ne 0) { throw "Could not replace the existing OneCopy runtime." }
    Write-Step "Launching the existing built app (built: $builtAt)"
    Write-Host "If you changed source since then, run rebuild instead."

    # GUI app: launch non-blocking via Start-Process.
    Start-Process -FilePath $exePath | Out-Null
    & node $runtimeHelper wait-process $exePath 30000
    if ($LASTEXITCODE -ne 0) { throw "OneCopy did not start." }
}
catch {
    Write-Host ""
    Write-Host "onecopy run-built failed: $($_.Exception.Message)" -ForegroundColor Red
    $scriptExitCode = 1
}
finally {
    Read-Host "Press Enter to close" | Out-Null
}

exit $scriptExitCode
