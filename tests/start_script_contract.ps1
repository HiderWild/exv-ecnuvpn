$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$start = Get-Content -LiteralPath (Join-Path $repoRoot 'start.ps1') -Raw
$restartPath = Join-Path $repoRoot 'scripts\restart-debug-electron.ps1'

function Require-Contains {
  param(
    [Parameter(Mandatory = $true)][string]$Text,
    [Parameter(Mandatory = $true)][string]$Needle,
    [Parameter(Mandatory = $true)][string]$Message
  )

  if (-not $Text.Contains($Needle)) {
    throw $Message
  }
}

Require-Contains $start 'param(' 'start.ps1 must define parameterized modes'
Require-Contains $start '[switch]$Quick' 'start.ps1 must expose -Quick'
Require-Contains $start '[switch]$NoFrontendBuild' 'start.ps1 must expose -NoFrontendBuild'
Require-Contains $start '[switch]$NoLaunch' 'start.ps1 must expose -NoLaunch'
Require-Contains $start '[switch]$PackageDir' 'start.ps1 must expose -PackageDir'
Require-Contains $start '[switch]$Package' 'start.ps1 must expose -Package'
Require-Contains $start '[switch]$CleanOnly' 'start.ps1 must expose -CleanOnly'
Require-Contains $start '[switch]$Status' 'start.ps1 must expose -Status'
Require-Contains $start 'cmake --preset windows-release' 'start.ps1 must configure C++ from preset'
Require-Contains $start 'cmake --build --preset windows-release' 'start.ps1 must build C++ from preset'
Require-Contains $start 'pnpm run prepare:native' 'start.ps1 must stage native binaries through prepare:native'
Require-Contains $start 'pnpm run desktop:package:dir' 'start.ps1 must support unpacked packaging'
Require-Contains $start 'pnpm run desktop:package' 'start.ps1 must support full packaging'
Require-Contains $start 'build-windows' 'start.ps1 must target build-windows outputs'
Require-Contains $start 'Get-FileHash' 'start.ps1 must print binary hashes for consistency checks'
Require-Contains $start 'desktop-rpc service.status' 'start.ps1 must probe runtime-selected native binary behavior'
if (Test-Path -LiteralPath $restartPath) {
  throw 'restart-debug-electron.ps1 has been retired; use root start.ps1 directly'
}

Write-Output 'start script contract passed.'
