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
Require-Contains $start 'windows-release' 'start.ps1 must configure C++ from the Windows preset'
Require-Contains $start 'scripts\build-windows.ps1 desktop' 'start.ps1 must build the native WebView desktop package'
Require-Contains $start 'package_ui_shell.py' 'start.ps1 must verify the native WebView package layout'
Require-Contains $start 'build\windows\webview\package\ECNU VPN' 'start.ps1 must use the native WebView package root'
Require-Contains $start 'exv-ui.exe' 'start.ps1 must launch the native WebView shell'
Require-Contains $start 'build-windows' 'start.ps1 must target build-windows outputs'
Require-Contains $start 'Get-FileHash' 'start.ps1 must print binary hashes for consistency checks'
Require-Contains $start 'desktop-rpc service.status' 'start.ps1 must probe runtime-selected native binary behavior'
Require-Contains $start 'Get-CimInstance Win32_Service' 'start.ps1 must detect the installed helper service through SCM'
Require-Contains $start 'Stop-Service -Name $helperServiceName' 'start.ps1 must stop the helper service before rebuilding'
Require-Contains $start 'sc.exe delete $helperServiceName' 'start.ps1 must uninstall the helper service before rebuilding'
Require-Contains $start 'Wait-HelperServiceAbsent' 'start.ps1 must wait for service deletion before deleting packaged binaries'
Require-Contains $start 'Invoke-HelperServicePrepackageCleanup' 'start.ps1 must expose a pre-package service cleanup step'
if ($start -notmatch 'Stop-ProjectProcesses\s+Invoke-HelperServicePrepackageCleanup\s+if \(-not \$Quick\)') {
  throw 'start.ps1 must run helper service cleanup after process cleanup and before deleting build artifacts'
}
if ($start -match 'desktop:package|build:electron|dist-electron|Find-ElectronProcess') {
  throw 'start.ps1 must not use retired Electron package or process paths'
}
if (Test-Path -LiteralPath $restartPath) {
  throw 'restart-debug-electron.ps1 has been retired; use root start.ps1 directly'
}

Write-Output 'start script contract passed.'
