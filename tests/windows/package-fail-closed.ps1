# Windows packaging test, run by `npm run test:full` on Windows and by the
# release workflow: `pwsh tests/windows/package-fail-closed.ps1`. It runs a copy
# of scripts/package.ps1 in a temporary sandbox against fake `node` and `tauri`
# commands, and proves that a failed version read or build removes stale
# outputs instead of shipping them, and that a successful build collects the
# installer and a portable zip holding only the executable and LICENSE.
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

    # A build that succeeds yields the installer and a portable zip that
    # carries the application licence beside the executable.
    Copy-Item (Join-Path $Repo "LICENSE") (Join-Path $Sandbox "LICENSE")
    Write-Command -Path $FakeNode -Lines @("@echo off", "echo 0.1.0", "exit /b 0")
    Write-Command -Path $FakeTauri -Lines @(
        "@echo off",
        "mkdir src-tauri\target\release\bundle\nsis 2>nul",
        "echo setup> src-tauri\target\release\bundle\nsis\OneCopy_0.1.0_x64-setup.exe",
        "echo portable> src-tauri\target\release\onecopy.exe",
        "exit /b 0"
    )
    & pwsh -NoProfile -NonInteractive -File (Join-Path $Sandbox "scripts/package.ps1") | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Package script failed for a successful build."
    }
    if (-not (Test-Path -PathType Leaf (Join-Path $Sandbox "artifacts/OneCopy-0.1.0-setup.exe"))) {
        throw "Package script did not collect the setup.exe."
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Zip = [System.IO.Compression.ZipFile]::OpenRead((Join-Path $Sandbox "artifacts/OneCopy-0.1.0-win.zip"))
    try {
        $Entries = @($Zip.Entries | ForEach-Object { $_.FullName } | Sort-Object)
    }
    finally {
        $Zip.Dispose()
    }
    if (($Entries -join ",") -ne "LICENSE,onecopy.exe") {
        throw "Portable zip must hold onecopy.exe and LICENSE; found: $($Entries -join ', ')"
    }

    Write-Host "Windows package handling passed."
}
finally {
    $env:Path = $OriginalPath
    Remove-Item -Recurse -Force $Sandbox -ErrorAction SilentlyContinue
}
