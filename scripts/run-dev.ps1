Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$scriptExitCode = 0

# run-dev: run the app from source with live reload, in its loosest configuration.
# For active coding and debugging. The strict, production-faithful launchers are
# run-built (launch the existing built app without rebuilding) and rebuild
# (build a fresh release binary, then launch).

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
$runtimeHelper = Join-Path $scriptDir "launcher-runtime.mjs"
$runtimeToken = [guid]::NewGuid().ToString("N")
$devHost = if ($env:TAURI_DEV_HOST) { $env:TAURI_DEV_HOST } else { "127.0.0.1" }
$devUrl = "http://${devHost}:28867"
$sourceReadyTimeoutMs = 900000

try {
    Set-Utf8Console
    Require-Command node
    Require-Command npm
    Require-Command cargo
    Require-Command rustc

    Set-Location $repoDir

    Write-Step "Replacing any existing OneCopy runtime"
    Invoke-Native -FilePath "node" -ArgumentList @($runtimeHelper, "claim", $runtimeToken)
    Invoke-Native -FilePath "node" -ArgumentList @($runtimeHelper, "stop", "tauri", "OneCopy", "onecopy")
    Invoke-Native -FilePath "node" -ArgumentList @($runtimeHelper, "check-endpoint", $devHost, "28867")
    if ($env:TAURI_DEV_HOST) {
        Invoke-Native -FilePath "node" -ArgumentList @($runtimeHelper, "check-endpoint", $devHost, "29587")
    }

    Write-Step "Installing dependencies required for launch"
    Invoke-Native -FilePath "npm" -ArgumentList @("install", "--no-audit", "--no-fund")

    Write-Step "Starting OneCopy in development mode"
    $devProcess = Start-Process -FilePath (Get-Command "npm.cmd").Source -ArgumentList @("run", "tauri", "dev") -NoNewWindow -PassThru
    Invoke-Native -FilePath "node" -ArgumentList @($runtimeHelper, "wait-http", $devUrl, "60000")
    Invoke-Native -FilePath "node" -ArgumentList @($runtimeHelper, "wait-process", (Join-Path $repoDir "src-tauri/target/debug/onecopy.exe"), $sourceReadyTimeoutMs)
    Write-Step "OneCopy is ready at $devUrl"
    $devProcess.WaitForExit()
    if ($devProcess.ExitCode -notin @(0, 130, -1073741510)) {
        throw "OneCopy development runtime failed with exit code $($devProcess.ExitCode)."
    }
}
catch {
    Write-Host ""
    Write-Host "onecopy run-dev failed: $($_.Exception.Message)" -ForegroundColor Red
    $scriptExitCode = 1
}
finally {
    & node $runtimeHelper is-owner $runtimeToken *> $null
    if ($LASTEXITCODE -eq 0) {
        & node $runtimeHelper stop-if-owner $runtimeToken tauri "OneCopy" "onecopy" *> $null
        Read-Host "Press Enter to close" | Out-Null
    }
}

exit $scriptExitCode
