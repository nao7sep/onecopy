$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Sandbox = Join-Path ([System.IO.Path]::GetTempPath()) ("onecopy-package-test-" + [guid]::NewGuid())
$OriginalPath = $env:Path

function Write-Command {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string[]]$Lines
    )

    Set-Content -Path $Path -Value $Lines -Encoding Ascii
}

function Seed-StaleOutputs {
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "artifacts") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "src-tauri/target/release/bundle/nsis") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "src-tauri/target/release") | Out-Null
    Set-Content -Path (Join-Path $Sandbox "artifacts/stale.zip") -Value "stale"
    Set-Content -Path (Join-Path $Sandbox "src-tauri/target/release/bundle/nsis/stale-setup.exe") -Value "stale"
    Set-Content -Path (Join-Path $Sandbox "src-tauri/target/release/onecopy.exe") -Value "stale"
}

function Assert-StaleOutputsRemoved {
    $StalePaths = @(
        (Join-Path $Sandbox "artifacts/stale.zip"),
        (Join-Path $Sandbox "src-tauri/target/release/bundle/nsis/stale-setup.exe"),
        (Join-Path $Sandbox "src-tauri/target/release/onecopy.exe")
    )

    foreach ($Path in $StalePaths) {
        if (Test-Path $Path) {
            throw "Failed package invocation retained stale output: $Path"
        }
    }
}

function Invoke-PackageExpectFailure {
    $Output = & pwsh -NoProfile -NonInteractive -File (Join-Path $Sandbox "scripts/package.ps1") 2>&1
    if ($LASTEXITCODE -eq 0) {
        throw "Package script unexpectedly succeeded. Output: $Output"
    }
}

try {
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "scripts") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "node_modules/.bin") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "fake-bin") | Out-Null
    Copy-Item (Join-Path $Repo "scripts/package.ps1") (Join-Path $Sandbox "scripts/package.ps1")

    $FakeNode = Join-Path $Sandbox "fake-bin/node.cmd"
    $FakeTauri = Join-Path $Sandbox "node_modules/.bin/tauri.cmd"
    $env:Path = (Join-Path $Sandbox "fake-bin") + [System.IO.Path]::PathSeparator + $OriginalPath

    Seed-StaleOutputs
    Write-Command -Path $FakeNode -Lines @("@echo off", "exit /b 19")
    Write-Command -Path $FakeTauri -Lines @("@echo off", "exit /b 0")
    Invoke-PackageExpectFailure
    Assert-StaleOutputsRemoved

    Seed-StaleOutputs
    Write-Command -Path $FakeNode -Lines @("@echo off", "echo 0.1.0", "exit /b 0")
    Write-Command -Path $FakeTauri -Lines @("@echo off", "exit /b 23")
    Invoke-PackageExpectFailure
    Assert-StaleOutputsRemoved

    Write-Host "Windows package failure handling passed."
}
finally {
    $env:Path = $OriginalPath
    Remove-Item -Recurse -Force $Sandbox -ErrorAction SilentlyContinue
}
