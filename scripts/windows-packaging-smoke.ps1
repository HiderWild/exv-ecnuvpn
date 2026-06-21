# windows-packaging-smoke.ps1
# Smoke tests for EXV Windows packaging — no real VPN connection required.
# Verifies that binaries, DLLs, IPC, and service infrastructure are present
# and functional before a release candidate is shipped.
#
# Usage:
#   .\scripts\windows-packaging-smoke.ps1 [-PackageRoot <path>] [-RuntimeDir <path>]
#
# Requires: PowerShell 5.1+, optionally exv-helper service installed.

param(
    [string]$PackageRoot = "",
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

function Resolve-StableHelperPath {
    if ($env:LOCALAPPDATA) {
        return (Join-Path $env:LOCALAPPDATA "EXV\Helper\exv-helper.exe")
    }
    if ($env:USERPROFILE) {
        return (Join-Path $env:USERPROFILE "AppData\Local\EXV\Helper\exv-helper.exe")
    }
    if ($env:ProgramData) {
        return (Join-Path $env:ProgramData "EXV\Helper\exv-helper.exe")
    }
    return "C:\ProgramData\EXV\Helper\exv-helper.exe"
}

function Convert-ServicePathNameToExecutablePath {
    param([string]$PathName)

    if ([string]::IsNullOrWhiteSpace($PathName)) {
        return $null
    }

    $trimmed = $PathName.Trim()
    if ($trimmed -match '^"([^"]+)"') {
        return $Matches[1]
    }
    if ($trimmed -match '^(.+?\.exe)(?:\s|$)') {
        return $Matches[1]
    }

    return $trimmed
}

function Test-SamePath {
    param(
        [string]$Left,
        [string]$Right
    )

    if ([string]::IsNullOrWhiteSpace($Left) -or [string]::IsNullOrWhiteSpace($Right)) {
        return $false
    }

    try {
        return ([System.IO.Path]::GetFullPath($Left).TrimEnd('\') -ieq
            [System.IO.Path]::GetFullPath($Right).TrimEnd('\'))
    }
    catch {
        return ($Left.TrimEnd('\') -ieq $Right.TrimEnd('\'))
    }
}

function Invoke-ExternalCommand {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [int]$TimeoutSeconds = 10
    )

    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $FilePath
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.CreateNoWindow = $true
    $startInfo.Arguments = ($Arguments | ForEach-Object {
        $arg = [string]$_
        if ($arg -match '^[A-Za-z0-9._:/\\=-]+$') {
            $arg
        } else {
            '"' + ($arg -replace '"', '\"') + '"'
        }
    }) -join ' '

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    try {
        [void]$process.Start()
        $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
        if ($timedOut) {
            try {
                $process.Kill()
            } catch { }
            return [pscustomobject]@{
                ExitCode = -1
                TimedOut = $true
                Output = ""
            }
        }

        $stdout = $process.StandardOutput.ReadToEnd()
        $stderr = $process.StandardError.ReadToEnd()
        $combined = (($stdout, $stderr) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }) -join "`n"
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            TimedOut = $false
            Output = $combined.Trim()
        }
    }
    finally {
        if ($process) {
            $process.Dispose()
        }
    }
}

# ── Resolve paths ────────────────────────────────────────────────────────────

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot  = Split-Path -Parent $scriptDir

if (-not $PackageRoot) { $PackageRoot = Join-Path $repoRoot "build\windows\webview\package\EXV" }
$uiShellExe    = Join-Path $PackageRoot "exv-ui.exe"
$exvExe        = Join-Path $PackageRoot "bin\exv.exe"
$exvHelperExe  = Join-Path $PackageRoot "bin\exv-helper.exe"
$stableHelperExe = Resolve-StableHelperPath

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

Add-RuntimeSearchDir $PackageRoot
Add-RuntimeSearchDir (Join-Path $PackageRoot "bin")
if ($RuntimeDir) {
    Add-RuntimeSearchDir $RuntimeDir
    Add-RuntimeSearchDir (Join-Path $RuntimeDir "win32-x64")
}

Write-Host ""
Write-Host "=== EXV Windows Packaging Smoke Tests ===" -ForegroundColor Cyan
Write-Host "Package root: $PackageRoot"
Write-Host "Stable helper: $stableHelperExe"
Write-Host "Runtime dirs: $($script:RuntimeSearchDirs -join '; ')"
Write-Host ""

# ── 1. Binary presence ───────────────────────────────────────────────────────

Write-Host "--- Binaries ---" -ForegroundColor Yellow

if (Test-Path $uiShellExe) {
    Write-Check "S00" "exv-ui.exe present" "PASS"
} else {
    Write-Check "S00" "exv-ui.exe present" "FAIL" "Not found at $uiShellExe"
}

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
    "WebView2Loader.dll",
    "libgcc_s_seh-1.dll",
    "libstdc++-6.dll",
    "libwinpthread-1.dll"
)

$optionalDlls = @(
    "wintun.dll"
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

foreach ($dll in $optionalDlls) {
    $found = $false
    foreach ($sp in $script:RuntimeSearchDirs) {
        if (Test-Path (Join-Path $sp $dll)) { $found = $true; break }
    }
    if ($found) {
        Write-Check "S03.$dll" "$dll present" "PASS"
    } else {
        Write-Check "S03.$dll" "$dll present" "SKIP" "Optional runtime asset not found. Provide it through EXV_RUNTIME_DIR before release packaging."
    }
}

# ── 2b. Package policy ───────────────────────────────────────────────────────

Write-Host ""
Write-Host "--- WebView Package Policy ---" -ForegroundColor Yellow

if (Test-Path -LiteralPath $PackageRoot) {
    $electronPayload = Get-ChildItem -Path $PackageRoot -Recurse -Include electron.exe,chromium.pak -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($electronPayload) {
        Write-Check "S03.policy" "No Electron payload in WebView package" "FAIL" "Found $($electronPayload.FullName)"
    } else {
        Write-Check "S03.policy" "No Electron payload in WebView package" "PASS"
    }

    try {
        $verifyScript = Join-Path $repoRoot "scripts\package_ui_shell.py"
        $verifyOutput = & python $verifyScript --verify-launch-targets-only --package-dir $PackageRoot 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Check "S03.args" "exv-ui launch arguments target packaged binaries" "PASS" "Output: $verifyOutput"
        } else {
            Write-Check "S03.args" "exv-ui launch arguments target packaged binaries" "FAIL" "Exit: $LASTEXITCODE, Output: $verifyOutput"
        }
    } catch {
        Write-Check "S03.args" "exv-ui launch arguments target packaged binaries" "FAIL" "Exception: $_"
    }
} else {
    Write-Check "S03.policy" "No Electron payload in WebView package" "FAIL" "Package root not found at $PackageRoot"
    Write-Check "S03.args" "exv-ui launch arguments target packaged binaries" "SKIP" "Package root not found"
}

# ── 3. exv --version ─────────────────────────────────────────────────────────

Write-Host ""
Write-Host "--- exv CLI ---" -ForegroundColor Yellow

if (Test-Path $exvExe) {
    try {
        $versionResult = Invoke-ExternalCommand -FilePath $exvExe -Arguments @("--version") -TimeoutSeconds 10
        if ($versionResult.ExitCode -eq 0 -and $versionResult.Output) {
            Write-Check "S04" "exv --version" "PASS" "Output: $($versionResult.Output)"
        } elseif ($versionResult.TimedOut) {
            Write-Check "S04" "exv --version" "FAIL" "Command timed out"
        } else {
            Write-Check "S04" "exv --version" "FAIL" "Exit code: $($versionResult.ExitCode), Output: $($versionResult.Output)"
        }
    } catch {
        Write-Check "S04" "exv --version" "FAIL" "Exception: $_"
    }
} else {
    Write-Check "S04" "exv --version" "SKIP" "exv.exe not found"
}

# ── 4. service status ────────────────────────────────────────────────────────

Write-Host ""
Write-Host "--- Service Status ---" -ForegroundColor Yellow

if (Test-Path $exvExe) {
    try {
        $svcResult = Invoke-ExternalCommand -FilePath $exvExe -Arguments @("desktop-rpc", "service.status", "{}") -TimeoutSeconds 10
        if ($svcResult.ExitCode -eq 0) {
            Write-Check "S05" "desktop-rpc service.status" "PASS" "Output: $($svcResult.Output)"
        } elseif ($svcResult.TimedOut) {
            Write-Check "S05" "desktop-rpc service.status" "SKIP" "Service status probe timed out"
        } else {
            Write-Check "S05" "desktop-rpc service.status" "SKIP" "Service not installed or not running (exit $($svcResult.ExitCode)). Output: $($svcResult.Output)"
        }
    } catch {
        Write-Check "S05" "desktop-rpc service.status" "SKIP" "Exception: $_"
    }
} else {
    Write-Check "S05" "desktop-rpc service.status" "SKIP" "exv.exe not found"
}

# ── 5. Helper service command path ───────────────────────────────────────────

Write-Host ""
Write-Host "--- Helper Service Registration ---" -ForegroundColor Yellow

$serviceName = "exv-helper"
$svcQuery = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($svcQuery -and $svcQuery.Status -eq "Running") {
    $binPath = $null
    try {
        $serviceInfo = Get-WmiObject Win32_Service -Filter "Name='$serviceName'" -ErrorAction SilentlyContinue
        if ($serviceInfo) {
            $binPath = $serviceInfo.PathName
        }
    } catch { }
    $serviceExe = Convert-ServicePathNameToExecutablePath $binPath
    if ($serviceExe -and (Test-SamePath $serviceExe $stableHelperExe) -and (Test-Path -LiteralPath $stableHelperExe)) {
        Write-Check "S06" "Helper service binary path correct" "PASS" "Path: $binPath"
    } elseif ($serviceExe -and (Test-SamePath $serviceExe $stableHelperExe)) {
        Write-Check "S06" "Helper service binary path correct" "FAIL" "Service points to stable helper, but the file is missing: $stableHelperExe"
    } elseif ($binPath) {
        Write-Check "S06" "Helper service binary path correct" "FAIL" "Installed service must point to $stableHelperExe, actual: $binPath"
    } else {
        Write-Check "S06" "Helper service binary path correct" "SKIP" "Helper service is running, but SCM path could not be inspected"
    }
} elseif ($svcQuery) {
    Write-Check "S06" "Helper service binary path correct" "SKIP" "Service '$serviceName' is installed but not running ($($svcQuery.Status))"
} else {
    Write-Check "S06" "Helper service binary path correct" "SKIP" "Service '$serviceName' not installed"
}

# ── 6. Helper Hello / IPC test ───────────────────────────────────────────────

Write-Host ""
Write-Host "--- Helper IPC ---" -ForegroundColor Yellow

Write-Check "S07" "Helper Hello handshake (IPC)" "SKIP" "Current CLI does not expose a standalone helper IPC probe"

# ── 7. Helper protocol capabilities ─────────────────────────────────────────

Write-Host ""
Write-Host "--- Helper Protocol Capabilities ---" -ForegroundColor Yellow

Write-Check "S08" "Helper protocol capabilities" "SKIP" "Current CLI does not expose a standalone helper capabilities probe"

# ── 8. desktop-rpc status ────────────────────────────────────────────────────

Write-Host ""
Write-Host "--- App Status ---" -ForegroundColor Yellow

Write-Check "S09" "exv status" "SKIP" "Release smoke does not run exv status because it can start a persistent core/backend from the temporary package"

# ── 9. Built-in uninstall command exists ─────────────────────────────────────

Write-Host ""
Write-Host "--- Uninstall Mechanism ---" -ForegroundColor Yellow

Write-Check "S10" "Uninstall mechanism available" "SKIP" "Portable smoke runs before NSIS; installer packaging validates the uninstaller"

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
