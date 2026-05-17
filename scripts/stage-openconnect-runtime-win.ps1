<#
.SYNOPSIS
  Stages the OpenConnect runtime (binaries, DLLs, driver assets) into the
  runtime/win32-<arch> directory for desktop packaging.

.DESCRIPTION
  After staging, `webui/scripts/prepare-native.cjs` copies the runtime into
  `webui/native/bin/` where electron-builder picks it up as extraResources.

  Runtime file categories:

    REQUIRED (VPN will not connect without these):
      - openconnect.exe
      - libopenconnect-5.dll

    REQUIRED for Wintun tunnel mode (default on Windows):
      - wintun.dll

    OPTIONAL (TAP tunnel mode — legacy fallback):
      - tap-windows-installer.exe  OR  tap/OemVista.inf

  If required files are missing, the script throws. If optional files are
  missing, the script prints a warning but succeeds — the packaged app will
  still work but the corresponding feature will be unavailable.

.PARAMETER SourceDir
  Path to the openconnect-gui installation directory (contains openconnect.exe
  and its dependency DLLs).

.PARAMETER Arch
  Target architecture. Default: "x64".

.PARAMETER TapAssetPath
  Optional path to a TAP installer (.exe) or directory containing TAP INF
  files. Staged as runtime/win32-<arch>/tap/.

.PARAMETER WintunDllPath
  Optional path to wintun.dll. Staged as runtime/win32-<arch>/wintun.dll.

.PARAMETER SkipWintun
  Skip staging wintun.dll even if WintunDllPath is provided. Use this for
  builds that intentionally omit Wintun support.

.PARAMETER SkipTap
  Skip staging TAP assets even if TapAssetPath is provided. Use this for
  builds that intentionally omit TAP support.

.EXAMPLE
  .\stage-openconnect-runtime-win.ps1 -SourceDir "C:\Program Files\OpenConnect-GUI" -WintunDllPath C:\Downloads\wintun\bin\amd64\wintun.dll

.EXAMPLE
  .\stage-openconnect-runtime-win.ps1 -SourceDir "C:\Program Files\OpenConnect-GUI" -SkipTap
#>
param(
  [Parameter(Mandatory = $true)]
  [string]$SourceDir,
  [string]$Arch = "x64",
  [string]$TapAssetPath = "",
  [string]$WintunDllPath = "",
  [switch]$SkipWintun,
  [switch]$SkipTap
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$runtimeDir = Join-Path $repoRoot ("runtime\win32-" + $Arch)

if (-not (Test-Path $SourceDir)) {
  throw "SourceDir does not exist: $SourceDir"
}

New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null

# --- REQUIRED files (script throws if missing) ---
$required = @(
  "openconnect.exe",
  "libopenconnect-5.dll"
)

foreach ($name in $required) {
  $source = Join-Path $SourceDir $name
  if (-not (Test-Path $source)) {
    throw "Missing required runtime file: $source"
  }
  Copy-Item $source -Destination (Join-Path $runtimeDir $name) -Force
}

# Copy all DLLs from SourceDir (includes GnuTLS, libxml2, etc.)
Get-ChildItem -Path $SourceDir -Filter *.dll -File | ForEach-Object {
  Copy-Item $_.FullName -Destination (Join-Path $runtimeDir $_.Name) -Force
}

# --- Wintun (required for Wintun tunnel mode, the default on Windows) ---
$wintunStaged = $false
if (-not $SkipWintun -and $WintunDllPath) {
  Copy-Item $WintunDllPath -Destination (Join-Path $runtimeDir "wintun.dll") -Force
  $wintunStaged = $true
} elseif ($SkipWintun) {
  Write-Host "  Skipping Wintun (-SkipWintun specified)" -ForegroundColor Gray
}

# --- TAP assets (optional — legacy tunnel mode) ---
$tapStaged = $false
if (-not $SkipTap -and $TapAssetPath) {
  $tapTarget = Join-Path $runtimeDir "tap"
  New-Item -ItemType Directory -Force -Path $tapTarget | Out-Null
  if (Test-Path $TapAssetPath -PathType Container) {
    Copy-Item (Join-Path $TapAssetPath "*") -Destination $tapTarget -Recurse -Force
  } else {
    Copy-Item $TapAssetPath -Destination (Join-Path $runtimeDir (Split-Path $TapAssetPath -Leaf)) -Force
  }
  $tapStaged = $true
} elseif ($SkipTap) {
  Write-Host "  Skipping TAP assets (-SkipTap specified)" -ForegroundColor Gray
}

# Copy license/copying files if present
Get-ChildItem -Path $SourceDir -Include LICENSE*,COPYING* -File | ForEach-Object {
  Copy-Item $_.FullName -Destination (Join-Path $runtimeDir $_.Name) -Force
}

# --- Validation summary ---
Write-Host ""
Write-Host "=== Runtime Staging Summary ===" -ForegroundColor Cyan
Write-Host "  Output: $runtimeDir"
Write-Host "  REQUIRED files:" -ForegroundColor Green
foreach ($name in $required) {
  $status = if (Test-Path (Join-Path $runtimeDir $name)) { "OK" } else { "MISSING" }
  Write-Host "    $name : $status"
}
Write-Host "  WINTUN (for Wintun tunnel mode): $(if ($wintunStaged) { 'STAGED' } elseif ($SkipWintun) { 'SKIPPED' } else { 'NOT STAGED - Wintun tunnel will not work' })" -ForegroundColor $(if ($wintunStaged) { 'Green' } elseif ($SkipWintun) { 'Gray' } else { 'Yellow' })
Write-Host "  TAP assets (optional fallback):  $(if ($tapStaged) { 'STAGED' } elseif ($SkipTap) { 'SKIPPED' } else { 'not staged' })" -ForegroundColor $(if ($tapStaged) { 'Green' } elseif ($SkipTap) { 'Gray' } else { 'Gray' })

$stagedFiles = Get-ChildItem -Path $runtimeDir -File
Write-Host "  Total files staged: $($stagedFiles.Count)"
Write-Host ""

if (-not $wintunStaged -and -not $SkipWintun) {
  Write-Host "  WARNING: wintun.dll was not staged. The default Wintun tunnel mode" -ForegroundColor Yellow
  Write-Host "  will not work. Provide it with -WintunDllPath <path>." -ForegroundColor Yellow
  Write-Host ""
}

Write-Host "Staged OpenConnect runtime to $runtimeDir"