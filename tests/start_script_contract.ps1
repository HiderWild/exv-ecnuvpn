$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$start = Get-Content -LiteralPath (Join-Path $repoRoot 'start.ps1') -Raw
$commonPath = Join-Path $repoRoot 'scripts\start-common.ps1'
$privilegedPath = Join-Path $repoRoot 'scripts\start-privileged-maintenance.ps1'
$privilegedLaunchPath = Join-Path $repoRoot 'scripts\start-privileged-webview-shell.ps1'
$common = if (Test-Path -LiteralPath $commonPath) { Get-Content -LiteralPath $commonPath -Raw } else { '' }
$privileged = if (Test-Path -LiteralPath $privilegedPath) { Get-Content -LiteralPath $privilegedPath -Raw } else { '' }
$privilegedLaunch = if (Test-Path -LiteralPath $privilegedLaunchPath) { Get-Content -LiteralPath $privilegedLaunchPath -Raw } else { '' }
$combined = "$start`n$common`n$privileged`n$privilegedLaunch"
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
Require-Contains $start 'start-common.ps1' 'start.ps1 must load the shared start script library'
Require-Contains $start 'Invoke-PrivilegedMaintenanceScript' 'start.ps1 must delegate privileged cleanup to a child script'
Require-Contains $start 'Set-ProtectedProcessIds (Get-CurrentProcessAncestorIds)' 'start.ps1 must install parent protected PIDs through the shared setter'
Require-Contains $start 'Build-WebViewPackage' 'start.ps1 must run build/package work in the root non-privileged process'
Require-Contains $start 'Test-WebViewPackage' 'start.ps1 must verify the package from the root process'
Require-Contains $start 'Start-WebViewShell' 'start.ps1 must launch the shell from the root process'
Require-Contains $common 'windows-release' 'shared start script must configure C++ from the Windows preset'
Require-Contains $common 'scripts\build-windows.ps1 desktop' 'shared start script must build the native WebView desktop package'
Require-Contains $common 'package_ui_shell.py' 'shared start script must verify the native WebView package layout'
Require-Contains $common 'build\windows\webview\package\EXV' 'shared start script must use the native WebView package root'
Require-Contains $common 'exv-ui.exe' 'shared start script must launch the native WebView shell'
Require-Contains $common 'build-windows' 'shared start script must target build-windows outputs'
Require-Contains $common 'Get-FileHash' 'shared start script must print binary hashes for consistency checks'
Require-Contains $common 'desktop-rpc service.status' 'shared start script must probe runtime-selected native binary behavior'
Require-Contains $common 'Get-CimInstance Win32_Service' 'shared start script must detect the installed helper service through SCM'
Require-Contains $common 'Test-HelperServiceUsesStablePath' 'shared start script must distinguish stable helper services from stale package services'
Require-Contains $common 'EXV\Helper\exv-helper.exe' 'shared start script must know the stable user-local helper path'
Require-Contains $common 'Stop-StaleHelperService' 'shared start script must only stop helper services that are stale or broken'
Require-Contains $common 'Stop-Service -Name $helperServiceName' 'shared start script must still be able to stop stale helper services before rebuilding'
Require-Contains $common 'sc.exe delete $helperServiceName' 'shared start script must still be able to uninstall stale helper services before rebuilding'
Require-Contains $common 'Wait-HelperServiceAbsent' 'shared start script must wait for service deletion before deleting packaged binaries'
Require-Contains $common 'Invoke-HelperServicePrepackageCleanup' 'shared start script must expose a pre-package service cleanup step'
Require-Contains $common 'Set-ProtectedProcessIds' 'shared start script must expose an explicit protected PID setter'
Require-Contains $common 'Get-ProtectedProcessIds' 'shared start script must expose protected PID diagnostics'
Require-Contains $common '$global:ExvStartProtectedProcessIds' 'protected PID state must use process-global storage to avoid dot-source script-scope ambiguity'
Require-Contains $common 'ContainsKey($ProcessId)' 'protected PID checks must use hash lookup instead of PowerShell array containment'
Require-Contains $common '$protectedCsv = (@(Get-ProtectedProcessIds)' 'privileged child arguments must be serialized from the shared protected PID store'
Require-Contains $common 'Where-Object { -not (Test-IsProtectedProcessId ([int]$_.ProcessId)) }' 'project process enumeration must filter protected PIDs at the final result boundary'
Require-Contains $common 'uninstall skipped' 'shared start script must skip uninstall when the service already uses the stable helper'
Require-Contains $common '$env:EXV_CORE_PATH' 'shared start script must expose packaged bin directory to the UI core resolver during debug launch'
Require-Contains $common 'Split-Path -Parent $exvExe' 'shared start script must derive EXV_CORE_PATH from the packaged bin/exv.exe path'
Require-Contains $common 'Restore-LaunchEnvironment' 'shared start script must restore process environment after launching the debug UI'
Require-Contains $common 'Resolve-LaunchArgs' 'shared start script must resolve packaged launch arguments before debug launch'
Require-Contains $common 'Join-Path $packageRoot $value' 'shared start script must turn relative exv-ui.args paths into absolute package paths'
Require-Contains $common 'Join-LaunchArgsForStartProcess' 'shared start script must quote launch arguments before calling Start-Process'
Require-Contains $common "-ArgumentList `$launchArgumentList" 'shared start script must pass a single quoted argument string to Start-Process'
Require-Contains $common 'Invoke-PrivilegedWebViewShellScript' 'shared start script must launch the requireAdministrator WebView shell through a privileged child script'
Require-Contains $common 'start-privileged-webview-shell.ps1' 'shared start script must know the privileged WebView shell launcher path'
Require-Contains $privileged 'start-common.ps1' 'privileged maintenance child must load the shared start script library'
Require-Contains $privileged 'Start-Transcript' 'privileged maintenance child must capture its own log'
Require-Contains $privileged 'Stop-ProjectProcesses' 'privileged maintenance child must stop stale project processes'
Require-Contains $privileged 'Invoke-HelperServicePrepackageCleanup' 'privileged maintenance child must clean stale helper services'
Require-Contains $privileged 'ProtectedProcessIds' 'privileged maintenance child must protect the parent non-privileged shell'
Require-Contains $privileged 'Set-ProtectedProcessIds $ids' 'privileged maintenance child must install protected PIDs through the shared setter'
Require-Contains $privilegedLaunch 'start-common.ps1' 'privileged WebView shell launcher must load the shared start script library'
Require-Contains $privilegedLaunch 'Start-Transcript' 'privileged WebView shell launcher must capture its own log'
Require-Contains $privilegedLaunch 'Start-WebViewShellDirect' 'privileged WebView shell launcher must call the direct elevated launch implementation'
Require-Contains $privilegedLaunch 'WebView shell launcher finished' 'privileged WebView shell launcher must leave a successful transcript marker'
if ($start -match 'WindowsBuiltInRole\]::Administrator|Verb RunAs') {
  throw 'start.ps1 must not self-elevate the entire build; only privileged child scripts may request UAC'
}
if ($combined -match '\$script:protectedProcessIds') {
  throw 'protected PID state must not use script scope because dot-sourced child scripts can read a different script scope'
}
if ($common -notmatch "start-privileged-maintenance\.ps1") {
  throw 'shared start script must know the privileged maintenance child script path'
}
if ($common -notmatch "start-privileged-webview-shell\.ps1") {
  throw 'shared start script must know the privileged WebView shell child script path'
}
if ($start -notmatch 'Invoke-PrivilegedMaintenanceScript[\s\S]+if \(-not \$Quick\)[\s\S]+Build-WebViewPackage') {
  throw 'start.ps1 must run privileged maintenance before non-privileged cleanup and build'
}
if ($combined -match 'desktop:package|build:electron|dist-electron|Find-ElectronProcess') {
  throw 'start.ps1 must not use retired Electron package or process paths'
}
if (Test-Path -LiteralPath $restartPath) {
  throw 'restart-debug-electron.ps1 has been retired; use root start.ps1 directly'
}

. $commonPath -RepoRoot $repoRoot -Port 8288
Set-ProtectedProcessIds @(@(101, 102), '103,104', '105 106')
$protectedIds = @(Get-ProtectedProcessIds)
foreach ($candidatePid in @(101, 102, 103, 104, 105, 106)) {
  if (-not (Test-IsProtectedProcessId -ProcessId $candidatePid)) {
    throw "protected PID setter must flatten and preserve PID $candidatePid"
  }
}
if (($protectedIds -join ', ') -ne '101, 102, 103, 104, 105, 106') {
  throw "protected PID diagnostics must expose a flat sorted list, got: $($protectedIds -join ', ')"
}

Write-Output 'start script contract passed.'
