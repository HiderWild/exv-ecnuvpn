# packaging-smoke.ps1 — Windows Packaging Smoke Test
#
# Verifies that build output artifacts exist, binaries run, runtime DLLs are
# present, and packaging prerequisites are met. Intended to run after a full
# Windows build (cmake --preset windows-release + electron build).
#
# Usage:
#   .\scripts\packaging-smoke.ps1 [-BuildDir <path>] [-JsonOutput] [-Verbose]
#
# Exit codes:
#   0 — All checks passed
#   1 — One or more checks failed

param(
  [string]$BuildDir = "",
  [switch]$JsonOutput,
  [switch]$Verbose
)

$ErrorActionPreference = "Continue"

# --- Resolve project root ---
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir

# --- Auto-detect build directory ---
if (-not $BuildDir) {
  $candidates = @(
    (Join-Path $repoRoot "build-win-codex"),
    (Join-Path $repoRoot "build-windows\cpp"),
    (Join-Path $repoRoot "build\windows\cpp")
  )
  foreach ($c in $candidates) {
    if (Test-Path $c) {
      $BuildDir = $c
      break
    }
  }
}

$electronDir = Join-Path $repoRoot "webui"
$releaseDir = Join-Path $repoRoot "build\windows\electron\release"
$runtimeDir = Join-Path $repoRoot "runtime\win32-x64"

# --- Check results tracking ---
$results = @()
$failedCount = 0
$passedCount = 0

function Add-Check {
  param(
    [string]$Name,
    [bool]$Passed,
    [string]$Detail = ""
  )
  $script:results += @{
    check = $Name
    passed = $Passed
    detail = $Detail
  }
  if ($Passed) {
    $script:passedCount++
    if ($Verbose) { Write-Host "  PASS: $Name" -ForegroundColor Green }
  } else {
    $script:failedCount++
    Write-Host "  FAIL: $Name" -ForegroundColor Red
    if ($Detail) { Write-Host "        $Detail" -ForegroundColor DarkYellow }
  }
}

Write-Host ""
Write-Host "=== ECNU-VPN Windows Packaging Smoke ===" -ForegroundColor Cyan
Write-Host ""

# --- 1. Build directory exists ---
Add-Check "Build directory exists" (Test-Path $BuildDir) "Checked: $BuildDir"

# --- 2. exv.exe exists and is non-empty ---
$exvPath = Join-Path $BuildDir "exv.exe"
$exvExists = (Test-Path $exvPath) -and ((Get-Item $exvPath).Length -gt 0)
Add-Check "exv.exe exists" $exvExists "Path: $exvPath"

# --- 3. exv-helper.exe exists and is non-empty ---
$exvHelperPath = Join-Path $BuildDir "exv-helper.exe"
$exvHelperExists = (Test-Path $exvHelperPath) -and ((Get-Item $exvHelperPath).Length -gt 0)
Add-Check "exv-helper.exe exists" $exvHelperExists "Path: $exvHelperPath"

# --- 4. exv.exe --version works ---
$exvVersionOk = $false
$exvVersion = ""
if ($exvExists) {
  try {
    $exvVersion = & $exvPath --version 2>&1 | Out-String
    $exvVersionOk = $LASTEXITCODE -eq 0 -and $exvVersion.Length -gt 0
  } catch {
    $exvVersion = "ERROR: $($_.Exception.Message)"
  }
}
Add-Check "exv.exe --version works" $exvVersionOk $exvVersion.Trim()

# --- 5. exv-helper.exe --version or --help works ---
$exvHelperVersionOk = $false
$exvHelperVersion = ""
if ($exvHelperExists) {
  try {
    $exvHelperVersion = & $exvHelperPath --version 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
      $exvHelperVersion = & $exvHelperPath --help 2>&1 | Out-String
    }
    $exvHelperVersionOk = $LASTEXITCODE -eq 0 -and $exvHelperVersion.Length -gt 0
  } catch {
    $exvHelperVersion = "ERROR: $($_.Exception.Message)"
  }
}
Add-Check "exv-helper.exe --version works" $exvHelperVersionOk $exvHelperVersion.Trim()

# --- 6. Runtime DLLs present ---
$dllChecks = @(
  @{ name = "wintun.dll"; path = (Join-Path $runtimeDir "wintun.dll") }
)
foreach ($dll in $dllChecks) {
  $present = (Test-Path $dll.path) -and ((Get-Item $dll.path).Length -gt 0)
  Add-Check "Runtime DLL: $($dll.name)" $present "Path: $($dll.path)"
}

# --- 7. MinGW runtime DLLs in build directory ---
$mingwDlls = @("libstdc++-6.dll", "libgcc_s_seh-1.dll", "libwinpthread-1.dll")
$hasMingw = $false
foreach ($dll in $mingwDlls) {
  if (Test-Path (Join-Path $BuildDir $dll)) {
    $hasMingw = $true
  }
}
Add-Check "MinGW runtime DLLs in build dir" $hasMingw "Checked for: $($mingwDlls -join ', ')"

# --- 8. Electron packaging prerequisites ---
$packageJson = Join-Path $electronDir "package.json"
$packageJsonExists = Test-Path $packageJson
Add-Check "webui/package.json exists" $packageJsonExists

$electronMain = Join-Path $electronDir "dist-electron\main\index.js"
$electronMainExists = Test-Path $electronMain
Add-Check "Electron main bundle exists" $electronMainExists "Path: $electronMain"

$rendererIndex = Join-Path $electronDir "dist\index.html"
$rendererExists = Test-Path $rendererIndex
Add-Check "Renderer bundle exists" $rendererExists "Path: $rendererIndex"

# --- 9. NSIS installer script ---
$nsisInclude = Join-Path $electronDir "build-resources\installer.nsh"
$nsisExists = Test-Path $nsisInclude
Add-Check "NSIS installer script (build-resources/installer.nsh)" $nsisExists "Path: $nsisInclude (optional)"

# --- 10. Helper service registration script ---
$installScript = Join-Path $repoRoot "scripts\install-windows.bat"
$installScriptExists = Test-Path $installScript
Add-Check "Helper service registration script" $installScriptExists "Path: $installScript"

# --- 11. Release artifacts (if packaging was run) ---
$nsisArtifact = Get-ChildItem -Path $releaseDir -Filter "*.exe" -Recurse -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -match "nsis|installer" } | Select-Object -First 1
$portableArtifact = Get-ChildItem -Path $releaseDir -Filter "*.zip" -Recurse -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -match "portable" } | Select-Object -First 1

$nsisArtifactOk = $null -ne $nsisArtifact
$portableArtifactOk = $null -ne $portableArtifact

Add-Check "NSIS installer artifact exists" $nsisArtifactOk $(if ($nsisArtifactOk) { $nsisArtifact.FullName } else { "Not found in $releaseDir (run electron-builder first)" })
Add-Check "Portable artifact exists" $portableArtifactOk $(if ($portableArtifactOk) { $portableArtifact.FullName } else { "Not found in $releaseDir (run electron-builder first)" })

# --- Summary ---
Write-Host ""
Write-Host "=== Summary ===" -ForegroundColor Cyan
Write-Host "  Passed: $passedCount" -ForegroundColor Green
Write-Host "  Failed: $failedCount" -ForegroundColor $(if ($failedCount -gt 0) { "Red" } else { "Green" })
Write-Host ""

if ($JsonOutput) {
  $output = @{
    timestamp = (Get-Date -Format "o")
    platform = "windows"
    buildDir = $BuildDir
    passed = $passedCount
    failed = $failedCount
    checks = $results
  }
  $output | ConvertTo-Json -Depth 5 | Write-Output
}

if ($failedCount -gt 0) {
  Write-Host "RESULT: FAIL" -ForegroundColor Red
  exit 1
} else {
  Write-Host "RESULT: PASS" -ForegroundColor Green
  exit 0
}
