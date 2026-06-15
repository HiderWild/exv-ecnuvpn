# windows-packaging-smoke.ps1
# Smoke tests for ECNU-VPN Windows packaging — no real VPN connection required.
# Verifies that binaries, DLLs, IPC, and service infrastructure are present
# and functional before a release candidate is shipped.
#
# Usage:
#   .\scripts\windows-packaging-smoke.ps1 [-BuildDir <path>] [-RuntimeDir <path>]
#
# Requires: PowerShell 5.1+, optionally exv-helper service installed.

param(
    [string]$BuildDir = "",
    [string]$RuntimeDir = ""
)

$ErrorActionPreference = "Continue"

# ── Helpers ──────────────────────────────────────────────────────────────────

$script:PassCount = 0
$script:FailCount = 0
$script:SkipCount = 0

function Write-Check {
    param(
        [string]$Id,
        [string]$Name,
        [string]$Result,   # PASS, FAIL, SKIP
        [string]$Detail = ""
    )
    switch ($Result) {
        "PASS" { Write-Host "  [PASS] $Id  $Name" -ForegroundColor Green; $script:PassCount++ }
        "FAIL" { Write-Host "  [FAIL] $Id  $Name" -ForegroundColor Red; if ($Detail) { Write-Host "         $Detail" -ForegroundColor DarkYellow }; $script:FailCount++ }
        "SKIP" { Write-Host "  [SKIP] $Id  $Name" -ForegroundColor Yellow; if ($Detail) { Write-Host "         $Detail" -ForegroundColor DarkGray }; $script:SkipCount++ }
    }
}

# ── Resolve paths ────────────────────────────────────────────────────────────

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot  = Split-Path -Parent $scriptDir

if (-not $BuildDir)   { $BuildDir   = Join-Path $repoRoot "build" }
$exvExe        = Join-Path $BuildDir "exv.exe"
$exvHelperExe  = Join-Path $BuildDir "exv-helper.exe"

$script:RuntimeSearchDirs = New-Object System.Collections.Generic.List[string]
function Add-RuntimeSearchDir {
    param([string]$Path)
    if (-not $Path) { return }
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $resolved = (Resolve-Path -LiteralPath $Path).Path
    if (-not $script:RuntimeSearchDirs.Contains($resolved)) {
        [void]$script:RuntimeSearchDirs.Add($resolved)
    }
}

Add-RuntimeSearchDir $BuildDir
Add-RuntimeSearchDir (Join-Path $BuildDir "windows\electron\bin")
Add-RuntimeSearchDir (Join-Path $BuildDir "windows\electron\native\bin")
Add-RuntimeSearchDir (Join-Path $BuildDir "windows\bin")
if ($RuntimeDir) {
    Add-RuntimeSearchDir $RuntimeDir
    Add-RuntimeSearchDir (Join-Path $RuntimeDir "win32-x64")
}

Write-Host ""
Write-Host "=== ECNU-VPN Windows Packaging Smoke Tests ===" -ForegroundColor Cyan
Write-Host "Build dir:   $BuildDir"
Write-Host "Runtime dirs: $($script:RuntimeSearchDirs -join '; ')"
Write-Host ""

# ── 1. Binary presence ───────────────────────────────────────────────────────

Write-Host "--- Binaries ---" -ForegroundColor Yellow

if (Test-Path $exvExe) {
    Write-Check "S01" "exv.exe present" "PASS"
} else {
    Write-Check "S01" "exv.exe present" "FAIL" "Not found at $exvExe"
}

if (Test-Path $exvHelperExe) {
    Write-Check "S02" "exv-helper.exe present" "PASS"
} else {
    Write-Check "S02" "exv-helper.exe present" "FAIL" "Not found at $exvHelperExe"
}

# ── 2. Runtime DLLs ──────────────────────────────────────────────────────────

Write-Host ""
Write-Host "--- Runtime DLLs ---" -ForegroundColor Yellow

$requiredDlls = @(
    "wintun.dll",
    "libgcc_s_seh-1.dll",
    "libstdc++-6.dll",
    "libwinpthread-1.dll"
)

foreach ($dll in $requiredDlls) {
    # Check in build dir first, then runtime dir
    $found = $false
    foreach ($sp in $script:RuntimeSearchDirs) {
        if (Test-Path (Join-Path $sp $dll)) { $found = $true; break }
    }
    if ($found) {
        Write-Check "S03.$dll" "$dll present" "PASS"
    } else {
        Write-Check "S03.$dll" "$dll present" "FAIL" "Not found in runtime search dirs"
    }
}

# ── 3. exv --version ─────────────────────────────────────────────────────────

Write-Host ""
Write-Host "--- exv CLI ---" -ForegroundColor Yellow

if (Test-Path $exvExe) {
    try {
        $output = & $exvExe --version 2>&1
        $exitCode = $LASTEXITCODE
        if ($exitCode -eq 0 -and $output) {
            Write-Check "S04" "exv --version" "PASS" "Output: $output"
        } else {
            Write-Check "S04" "exv --version" "FAIL" "Exit code: $exitCode, Output: $output"
        }
    } catch {
        Write-Check "S04" "exv --version" "FAIL" "Exception: $_"
    }
} else {
    Write-Check "S04" "exv --version" "SKIP" "exv.exe not found"
}

# ── 4. exv service status ────────────────────────────────────────────────────

Write-Host ""
Write-Host "--- Service Status ---" -ForegroundColor Yellow

if (Test-Path $exvExe) {
    try {
        $svcOutput = & $exvExe service status 2>&1
        $svcExit = $LASTEXITCODE
        if ($svcExit -eq 0) {
            Write-Check "S05" "exv service status" "PASS" "Output: $svcOutput"
        } else {
            Write-Check "S05" "exv service status" "SKIP" "Service not installed or not running (exit $svcExit). Output: $svcOutput"
        }
    } catch {
        Write-Check "S05" "exv service status" "SKIP" "Exception: $_"
    }
} else {
    Write-Check "S05" "exv service status" "SKIP" "exv.exe not found"
}

# ── 5. Helper service command path ───────────────────────────────────────────

Write-Host ""
Write-Host "--- Helper Service Registration ---" -ForegroundColor Yellow

$serviceName = "exv-helper"
$svcQuery = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($svcQuery) {
    $binPath = (Get-WmiObject Win32_Service -Filter "Name='$serviceName'" -ErrorAction SilentlyContinue).PathName
    if ($binPath -and $binPath -match "exv-helper") {
        Write-Check "S06" "Helper service binary path correct" "PASS" "Path: $binPath"
    } else {
        Write-Check "S06" "Helper service binary path correct" "FAIL" "Unexpected path: $binPath"
    }
} else {
    Write-Check "S06" "Helper service binary path correct" "SKIP" "Service '$serviceName' not installed"
}

# ── 6. Helper Hello / IPC test ───────────────────────────────────────────────

Write-Host ""
Write-Host "--- Helper IPC ---" -ForegroundColor Yellow

if ($svcQuery -and $svcQuery.Status -eq "Running" -and (Test-Path $exvExe)) {
    try {
        $helloOutput = & $exvExe helper hello 2>&1
        $helloExit = $LASTEXITCODE
        if ($helloExit -eq 0 -and $helloOutput -match "Hello|capabilities") {
            Write-Check "S07" "Helper Hello handshake (IPC)" "PASS" "Output: $helloOutput"
        } else {
            Write-Check "S07" "Helper Hello handshake (IPC)" "FAIL" "Exit: $helloExit, Output: $helloOutput"
        }
    } catch {
        Write-Check "S07" "Helper Hello handshake (IPC)" "FAIL" "Exception: $_"
    }
} else {
    Write-Check "S07" "Helper Hello handshake (IPC)" "SKIP" "Helper service not running"
}

# ── 7. Helper protocol capabilities ─────────────────────────────────────────

Write-Host ""
Write-Host "--- Helper Protocol Capabilities ---" -ForegroundColor Yellow

if ($svcQuery -and $svcQuery.Status -eq "Running" -and (Test-Path $exvExe)) {
    try {
        $capsOutput = & $exvExe helper capabilities 2>&1
        $capsExit = $LASTEXITCODE
        $expectedCaps = @("tunnel_device_create", "route_apply", "dns_apply", "route_cleanup")
        if ($capsExit -eq 0) {
            $capsText = $capsOutput -join " "
            $missing = @()
            foreach ($cap in $expectedCaps) {
                if ($capsText -notmatch $cap) { $missing += $cap }
            }
            if ($missing.Count -eq 0) {
                Write-Check "S08" "Helper protocol capabilities" "PASS" "All expected capabilities present"
            } else {
                Write-Check "S08" "Helper protocol capabilities" "FAIL" "Missing: $($missing -join ', ')"
            }
        } else {
            Write-Check "S08" "Helper protocol capabilities" "FAIL" "Exit: $capsExit, Output: $capsOutput"
        }
    } catch {
        Write-Check "S08" "Helper protocol capabilities" "FAIL" "Exception: $_"
    }
} else {
    Write-Check "S08" "Helper protocol capabilities" "SKIP" "Helper service not running"
}

# ── 8. desktop-rpc status ────────────────────────────────────────────────────

Write-Host ""
Write-Host "--- Desktop RPC ---" -ForegroundColor Yellow

if (Test-Path $exvExe) {
    try {
        $rpcOutput = & $exvExe rpc status 2>&1
        $rpcExit = $LASTEXITCODE
        if ($rpcExit -eq 0) {
            Write-Check "S09" "desktop-rpc status" "PASS" "Output: $rpcOutput"
        } else {
            Write-Check "S09" "desktop-rpc status" "SKIP" "RPC not running or not available (exit $rpcExit). Output: $rpcOutput"
        }
    } catch {
        Write-Check "S09" "desktop-rpc status" "SKIP" "Exception: $_"
    }
} else {
    Write-Check "S09" "desktop-rpc status" "SKIP" "exv.exe not found"
}

# ── 9. Built-in uninstall command exists ─────────────────────────────────────

Write-Host ""
Write-Host "--- Uninstall Mechanism ---" -ForegroundColor Yellow

$foundUninstall = $false

if (Test-Path $exvExe) {
    try {
        $uninstallOutput = & $exvExe service uninstall --dry-run 2>&1
        $uninstallText = $uninstallOutput -join " "
        if ($LASTEXITCODE -eq 0 -or
            ($uninstallText -and $uninstallText -notmatch "Unknown command|Run 'exv help'")) {
            $foundUninstall = $true
        }
    } catch { }
}

if ($foundUninstall) {
    Write-Check "S10" "Uninstall mechanism available" "PASS"
} else {
    Write-Check "S10" "Uninstall mechanism available" "FAIL" "Built-in uninstall command is unavailable"
}

# ── Summary ──────────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "=== Smoke Test Summary ===" -ForegroundColor Cyan
Write-Host "  PASS: $script:PassCount" -ForegroundColor Green
Write-Host "  FAIL: $script:FailCount" -ForegroundColor $(if ($script:FailCount -gt 0) { "Red" } else { "DarkGray" })
Write-Host "  SKIP: $script:SkipCount" -ForegroundColor Yellow
Write-Host ""

if ($script:FailCount -gt 0) {
    Write-Host "RESULT: FAIL — $script:FailCount check(s) failed" -ForegroundColor Red
    exit 1
} elseif ($script:SkipCount -gt 0) {
    Write-Host "RESULT: PASS (with $script:SkipCount skipped)" -ForegroundColor Yellow
    exit 0
} else {
    Write-Host "RESULT: ALL PASS" -ForegroundColor Green
    exit 0
}
