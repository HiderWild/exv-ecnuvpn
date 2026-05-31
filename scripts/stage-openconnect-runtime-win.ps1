param(
  [Parameter(Mandatory = $true)]
  [string]$SourceDir,
  [string]$Arch = "x64",
  [string]$TapAssetPath = "",
  [string]$WintunDllPath = ""
)

$ErrorActionPreference = "Stop"

if ($env:ECNUVPN_LEGACY_OPENCONNECT_RUNTIME -ne "1") {
  throw "OpenConnect runtime staging is legacy diagnostic-only. Set ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1 to run this script."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$runtimeDir = Join-Path $repoRoot ("runtime\legacy-openconnect\win32-" + $Arch)

if (-not (Test-Path $SourceDir)) {
  throw "SourceDir does not exist: $SourceDir"
}

New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null

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

Get-ChildItem -Path $SourceDir -Filter *.dll -File | ForEach-Object {
  Copy-Item $_.FullName -Destination (Join-Path $runtimeDir $_.Name) -Force
}

if ($WintunDllPath) {
  Copy-Item $WintunDllPath -Destination (Join-Path $runtimeDir "wintun.dll") -Force
}

if ($TapAssetPath) {
  $tapTarget = Join-Path $runtimeDir "tap"
  New-Item -ItemType Directory -Force -Path $tapTarget | Out-Null
  if (Test-Path $TapAssetPath -PathType Container) {
    Copy-Item (Join-Path $TapAssetPath "*") -Destination $tapTarget -Recurse -Force
  } else {
    Copy-Item $TapAssetPath -Destination (Join-Path $runtimeDir (Split-Path $TapAssetPath -Leaf)) -Force
  }
}

Get-ChildItem -Path $SourceDir -Include LICENSE*,COPYING* -File | ForEach-Object {
  Copy-Item $_.FullName -Destination (Join-Path $runtimeDir $_.Name) -Force
}

Write-Host "Staged legacy diagnostic OpenConnect runtime to $runtimeDir"
