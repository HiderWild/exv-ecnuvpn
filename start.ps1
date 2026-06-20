param(
  [switch]$Quick,
  [switch]$NoFrontendBuild,
  [switch]$NoLaunch,
  [switch]$PackageDir,
  [switch]$Package,
  [switch]$CleanOnly,
  [switch]$Status,
  [switch]$PauseOnError,
  [string]$WebView2SdkDir = $env:WEBVIEW2_SDK_DIR,
  [int]$Port = 8288
)

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  $scriptPath = $MyInvocation.MyCommand.Path
  $argString = ''
  foreach ($key in $PSBoundParameters.Keys) {
    $val = $PSBoundParameters[$key]
    if ($val -is [switch]) {
      if ($val) { $argString += " -$key" }
    }
    else {
      $argString += " -$key `"$val`""
    }
  }
  $argString += ' -PauseOnError'

  Write-Output '[start] Not running as Administrator; requesting elevation via UAC...'
  $elevated = Start-Process -FilePath 'powershell.exe' `
    -ArgumentList "-NoProfile -NoLogo -ExecutionPolicy Bypass -File `"$scriptPath`"$argString" `
    -Verb RunAs -Wait -PassThru
  exit $elevated.ExitCode
}

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$scriptDir = Join-Path $repoRoot 'scripts'
$webviewRoot = Join-Path $repoRoot 'build\windows\webview'
$packageRoot = Join-Path $repoRoot 'build\windows\webview\package\ECNU VPN'
$cppBuildRoot = Join-Path $repoRoot 'build-windows'
$cppBuildDir = Join-Path $cppBuildRoot 'cpp'
$uiShellExe = Join-Path $packageRoot 'exv-ui.exe'
$exvExe = Join-Path $packageRoot 'bin\exv.exe'
$exvHelperExe = Join-Path $packageRoot 'bin\exv-helper.exe'
$helperServiceName = 'exv-helper'
$launchArgsFile = Join-Path $packageRoot 'exv-ui.args'
$desktopOutLog = Join-Path $repoRoot 'build\windows\webview\start-desktop.out.log'
$desktopErrLog = Join-Path $repoRoot 'build\windows\webview\start-desktop.err.log'
$protectedProcessIds = @()

function Write-Info([string]$Message) {
  Write-Output "[start] $Message"
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

function Get-HelperServiceSnapshot {
  $service = Get-CimInstance Win32_Service -Filter "Name='$helperServiceName'" -ErrorAction SilentlyContinue
  if (-not $service) {
    return $null
  }

  $binaryPath = Convert-ServicePathNameToExecutablePath $service.PathName
  $binaryExists = $false
  if (-not [string]::IsNullOrWhiteSpace($binaryPath)) {
    $binaryExists = Test-Path -LiteralPath $binaryPath
  }

  return [pscustomobject]@{
    Name = $service.Name
    State = $service.State
    Status = $service.Status
    StartMode = $service.StartMode
    ProcessId = [int]$service.ProcessId
    PathName = $service.PathName
    BinaryPath = $binaryPath
    BinaryExists = $binaryExists
  }
}

function Write-HelperServiceSnapshot {
  param($Snapshot)

  if (-not $Snapshot) {
    Write-Info "Service $helperServiceName is not installed"
    return
  }

  $binarySummary = 'unknown'
  if (-not [string]::IsNullOrWhiteSpace($Snapshot.BinaryPath)) {
    $binarySummary = "$($Snapshot.BinaryPath) (exists=$($Snapshot.BinaryExists))"
  }

  Write-Info "Service ${helperServiceName}: state=$($Snapshot.State), start=$($Snapshot.StartMode), pid=$($Snapshot.ProcessId), binary=$binarySummary"
}

function Wait-HelperServiceStopped {
  param([int]$TimeoutSeconds = 10)

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    $snapshot = Get-HelperServiceSnapshot
    if (-not $snapshot -or $snapshot.State -eq 'Stopped') {
      return $true
    }
    Start-Sleep -Milliseconds 300
  }

  return $false
}

function Wait-HelperServiceAbsent {
  param([int]$TimeoutSeconds = 10)

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    if (-not (Get-HelperServiceSnapshot)) {
      return $true
    }
    Start-Sleep -Milliseconds 300
  }

  return $false
}

function Resolve-WebView2Sdk {
  param([string]$RequestedPath)

  if (-not [string]::IsNullOrWhiteSpace($RequestedPath)) {
    if (Test-Path -LiteralPath (Join-Path $RequestedPath 'build\native\include\WebView2.h')) {
      return (Resolve-Path -LiteralPath $RequestedPath).Path
    }
    throw "WEBVIEW2_SDK_DIR does not contain build\native\include\WebView2.h: $RequestedPath"
  }

  $candidate = Join-Path $repoRoot 'build\deps\webview2\1.0.4022.49'
  if (Test-Path -LiteralPath (Join-Path $candidate 'build\native\include\WebView2.h')) {
    return (Resolve-Path -LiteralPath $candidate).Path
  }

  throw 'WEBVIEW2_SDK_DIR is required, or build\deps\webview2\1.0.4022.49 must exist.'
}

function Invoke-Step {
  param(
    [Parameter(Mandatory = $true)][string]$Command,
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments,
    [string]$WorkingDirectory = $repoRoot
  )

  Push-Location $WorkingDirectory
  try {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
      throw "Command failed: $Command $($Arguments -join ' ')"
    }
  }
  finally {
    Pop-Location
  }
}

function Get-CurrentProcessAncestorIds {
  $processes = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue
  $parents = @{}
  foreach ($process in $processes) {
    $parents[[int]$process.ProcessId] = [int]$process.ParentProcessId
  }

  $ids = @()
  $seen = New-Object 'System.Collections.Generic.HashSet[int]'
  $current = [int]$PID
  while ($true) {
    if (-not $seen.Add($current)) { break }
    $ids += $current
    if (-not $parents.ContainsKey($current)) { break }
    $parent = $parents[$current]
    if ($parent -le 0) { break }
    $current = $parent
  }

  return $ids
}

function Test-IsProtectedProcessId {
  param([int]$ProcessId)
  return $protectedProcessIds -contains $ProcessId
}

function Get-ProcessesOnPort {
  param([int]$LocalPort)

  $owners = Get-NetTCPConnection -LocalPort $LocalPort -State Listen -ErrorAction SilentlyContinue |
    Select-Object -ExpandProperty OwningProcess -Unique
  if (-not $owners) {
    return @()
  }

  return Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessId -in $owners }
}

function Get-ProjectProcesses {
  $results = @()
  $alwaysMatchNames = @('exv.exe', 'exv-helper.exe', 'exv-ui.exe')
  $conditionalNames = @('node.exe', 'pnpm.exe', 'cmd.exe', 'powershell.exe', 'pwsh.exe')
  $commandPatterns = @(
    "*$repoRoot*",
    "*$packageRoot*",
    '*exv-ui.exe*',
    '*exv-helper.exe*',
    '*exv.exe*',
    "*localhost:$Port*",
    "*--port $Port*"
  )

  $results += Get-ProcessesOnPort -LocalPort $Port

  foreach ($process in Get-CimInstance Win32_Process -ErrorAction SilentlyContinue) {
    $processIdValue = [int]$process.ProcessId
    if (Test-IsProtectedProcessId $processIdValue) {
      continue
    }

    if ($alwaysMatchNames -contains $process.Name) {
      $results += $process
      continue
    }

    if ($conditionalNames -notcontains $process.Name) {
      continue
    }

    $commandLine = [string]$process.CommandLine
    if ([string]::IsNullOrWhiteSpace($commandLine)) {
      continue
    }

    foreach ($pattern in $commandPatterns) {
      if ($commandLine -like $pattern) {
        $results += $process
        break
      }
    }
  }

  return $results | Sort-Object ProcessId -Unique
}

function Stop-ProcessTreeSafe {
  param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [string]$Name = ''
  )

  if (Test-IsProtectedProcessId $ProcessId) {
    return
  }

  if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
    return
  }

  try {
    Stop-Process -Id $ProcessId -Force -ErrorAction Stop
  }
  catch {
    try {
      taskkill /PID $ProcessId /F /T | Out-Null
    }
    catch {
      $displayName = if ($Name) { $Name } else { "PID $ProcessId" }
      Write-Info "Failed to terminate ${displayName}: $($_.Exception.Message)"
    }
  }
}

function Stop-HelperService {
  $snapshot = Get-HelperServiceSnapshot
  if (-not $snapshot) {
    return
  }

  Write-HelperServiceSnapshot $snapshot
  if ($snapshot.State -eq 'Stopped') {
    return
  }

  Write-Info "Stopping service $helperServiceName"
  try {
    Stop-Service -Name $helperServiceName -Force -ErrorAction Stop
  }
  catch {
    try {
      sc.exe stop $helperServiceName | Out-Null
    }
    catch {
      Write-Info "Failed to stop service ${helperServiceName}: $($_.Exception.Message)"
    }
  }

  if (-not (Wait-HelperServiceStopped -TimeoutSeconds 10)) {
    $remaining = Get-HelperServiceSnapshot
    Write-HelperServiceSnapshot $remaining
    throw "Service $helperServiceName did not stop before rebuild."
  }
}

function Uninstall-HelperService {
  $snapshot = Get-HelperServiceSnapshot
  if (-not $snapshot) {
    Write-Info "Service $helperServiceName is not installed; uninstall skipped"
    return
  }

  Stop-HelperService

  Write-Info "Deleting service $helperServiceName"
  $deleteOutput = & sc.exe delete $helperServiceName 2>&1
  $deleteExitCode = $LASTEXITCODE
  if ($deleteExitCode -ne 0 -and (Get-HelperServiceSnapshot)) {
    $deleteText = ($deleteOutput | Out-String).Trim()
    throw "Failed to delete service $helperServiceName with exit code $deleteExitCode. $deleteText"
  }

  if (-not (Wait-HelperServiceAbsent -TimeoutSeconds 10)) {
    $remaining = Get-HelperServiceSnapshot
    Write-HelperServiceSnapshot $remaining
    throw "Service $helperServiceName is still registered after delete request. Close service management tools and rerun start.ps1."
  }

  Write-Info "Service $helperServiceName registration removed"
}

function Invoke-HelperServicePrepackageCleanup {
  Write-Info 'Checking helper service before package cleanup'
  Write-HelperServiceSnapshot (Get-HelperServiceSnapshot)
  Uninstall-HelperService
}

function Stop-ProjectProcesses {
  Write-Info 'Stopping residual project processes'
  Stop-HelperService

  foreach ($candidate in Get-ProjectProcesses) {
    Write-Info "Stopping $($candidate.Name)[$($candidate.ProcessId)]"
    Stop-ProcessTreeSafe -ProcessId ([int]$candidate.ProcessId) -Name $candidate.Name
  }

  $deadline = (Get-Date).AddSeconds(5)
  while ((Get-Date) -lt $deadline) {
    $remaining = Get-ProjectProcesses
    if (-not $remaining) {
      return
    }
    Start-Sleep -Milliseconds 300
  }

  $remaining = Get-ProjectProcesses
  if ($remaining) {
    $summary = ($remaining | ForEach-Object { "$($_.Name)[$($_.ProcessId)]" }) -join ', '
    throw "Residual project processes still running: $summary. Re-run from an elevated PowerShell terminal before rebuilding."
  }
}

function Remove-DirectorySafe {
  param([Parameter(Mandatory = $true)][string]$Path)

  if (Test-Path -LiteralPath $Path) {
    Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
  }
}

function Clean-BuildArtifacts {
  Write-Info 'Removing native WebView build and package artifacts'
  Remove-DirectorySafe $cppBuildRoot
  Remove-DirectorySafe $webviewRoot
}

function Show-FileHashIfPresent {
  param([Parameter(Mandatory = $true)][string]$Path)

  if (-not (Test-Path -LiteralPath $Path)) {
    Write-Info "Missing: $Path"
    return
  }

  Get-FileHash -Algorithm SHA256 -LiteralPath $Path |
    Select-Object Path, Hash |
    Format-Table -AutoSize
}

function Invoke-DesktopRpcProbe {
  param([Parameter(Mandatory = $true)][string]$ExecutablePath)

  if (-not (Test-Path -LiteralPath $ExecutablePath)) {
    Write-Info "Probe skipped because executable is missing: $ExecutablePath"
    return
  }

  Write-Info "Probe: $ExecutablePath desktop-rpc service.status '{}'"
  & $ExecutablePath 'desktop-rpc' 'service.status' '{}'
  Write-Info "Probe exit code: $LASTEXITCODE"
}

function Show-Status {
  Write-Info 'Helper service registration'
  Write-HelperServiceSnapshot (Get-HelperServiceSnapshot)

  Write-Info 'Native CMake outputs'
  Show-FileHashIfPresent (Join-Path $cppBuildDir 'exv-ui.exe')
  Show-FileHashIfPresent (Join-Path $cppBuildDir 'exv.exe')
  Show-FileHashIfPresent (Join-Path $cppBuildDir 'exv-helper.exe')

  Write-Info 'WebView package outputs'
  Show-FileHashIfPresent $uiShellExe
  Show-FileHashIfPresent $exvExe
  Show-FileHashIfPresent $exvHelperExe

  Invoke-DesktopRpcProbe $exvExe
}

function Build-WebViewPackage {
  $resolvedWebView2Sdk = Resolve-WebView2Sdk $WebView2SdkDir

  if ($NoFrontendBuild) {
    Write-Info 'Building native UI shell and packaging existing WebView renderer assets'
    Invoke-Step -Command 'cmake' -Arguments @('--preset', 'windows-release', '-DEXV_BUILD_UI_SHELL=ON', "-DWEBVIEW2_SDK_DIR=$resolvedWebView2Sdk")
    Invoke-Step -Command 'cmake' -Arguments @(
      '--build', '--preset', 'windows-release',
      '--target', 'exv', 'exv-helper', 'exv-ui'
    )
    Invoke-Step -Command 'python' -Arguments @(
      (Join-Path $scriptDir 'package_ui_shell.py'),
      '--platform', 'windows'
    )
    return
  }

  Write-Info 'Running: scripts\build-windows.ps1 desktop'
  Invoke-Step -Command 'powershell' -Arguments @(
    '-ExecutionPolicy', 'Bypass',
    '-File', (Join-Path $scriptDir 'build-windows.ps1'),
    'desktop',
    '-WebView2SdkDir', $resolvedWebView2Sdk
  )
}

function Test-WebViewPackage {
  $missing = @()
  foreach ($path in @($uiShellExe, $exvExe, $exvHelperExe, $launchArgsFile)) {
    if (-not (Test-Path -LiteralPath $path)) {
      $missing += $path
    }
  }
  if ($missing.Count -gt 0) {
    throw "WebView package is incomplete. Missing: $($missing -join ', ')"
  }

  Invoke-Step -Command 'python' -Arguments @(
    (Join-Path $scriptDir 'package_ui_shell.py'),
    '--verify-launch-targets-only',
    '--package-dir', $packageRoot
  )
}

function Set-LaunchEnvironment {
  param([Parameter(Mandatory = $true)][string]$CoreDirectory)

  $snapshot = [pscustomobject]@{
    ExvCorePath = $env:EXV_CORE_PATH
    Path = $env:PATH
  }

  $env:EXV_CORE_PATH = $CoreDirectory
  if ([string]::IsNullOrWhiteSpace($env:PATH)) {
    $env:PATH = $CoreDirectory
  }
  elseif ($env:PATH -notlike "*$CoreDirectory*") {
    $env:PATH = "$CoreDirectory;$env:PATH"
  }

  Write-Info "Launch environment: EXV_CORE_PATH=$env:EXV_CORE_PATH"
  return $snapshot
}

function Restore-LaunchEnvironment {
  param($Snapshot)

  if (-not $Snapshot) {
    return
  }

  $env:EXV_CORE_PATH = $Snapshot.ExvCorePath
  $env:PATH = $Snapshot.Path
}

function Resolve-LaunchArgPath {
  param([Parameter(Mandatory = $true)][string]$value)

  if ([System.IO.Path]::IsPathRooted($value)) {
    return $value
  }

  return (Join-Path $packageRoot $value)
}

function Resolve-LaunchArgs {
  param([Parameter(Mandatory = $true)][string[]]$Tokens)

  $resolved = @()
  for ($i = 0; $i -lt $Tokens.Count; $i++) {
    $token = $Tokens[$i]
    $resolved += $token
    if (($token -eq '--exv' -or $token -eq '--renderer-index') -and $i + 1 -lt $Tokens.Count) {
      $i++
      $resolved += (Resolve-LaunchArgPath $Tokens[$i])
    }
  }

  return @($resolved)
}

function ConvertTo-WindowsProcessArgument {
  param([AllowEmptyString()][string]$Value)

  if ($null -eq $Value) {
    return '""'
  }

  if ($Value -notmatch '[\s"]') {
    return $Value
  }

  $escaped = $Value -replace '\\(?=($|"))', '\\' -replace '"', '\"'
  return '"' + $escaped + '"'
}

function Join-LaunchArgsForStartProcess {
  param([Parameter(Mandatory = $true)][string[]]$Tokens)

  return (($Tokens | ForEach-Object { ConvertTo-WindowsProcessArgument $_ }) -join ' ')
}

function Read-LaunchArgs {
  if (-not (Test-Path -LiteralPath $launchArgsFile)) {
    throw "Launch argument file is missing: $launchArgsFile"
  }

  $tokens = @(Get-Content -LiteralPath $launchArgsFile |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
  return @(Resolve-LaunchArgs $tokens)
}

function Start-WebViewShell {
  Test-WebViewPackage

  if (Test-Path -LiteralPath $desktopOutLog) { Remove-Item -LiteralPath $desktopOutLog -Force -ErrorAction SilentlyContinue }
  if (Test-Path -LiteralPath $desktopErrLog) { Remove-Item -LiteralPath $desktopErrLog -Force -ErrorAction SilentlyContinue }

  $launchArgs = @(Read-LaunchArgs)
  $launchArgumentList = Join-LaunchArgsForStartProcess $launchArgs
  $launchEnvironment = Set-LaunchEnvironment -CoreDirectory (Split-Path -Parent $exvExe)
  Write-Info "Launching native WebView shell: $uiShellExe"
  try {
    $process = Start-Process -FilePath $uiShellExe `
      -ArgumentList $launchArgumentList `
      -WorkingDirectory $packageRoot `
      -RedirectStandardOutput $desktopOutLog `
      -RedirectStandardError $desktopErrLog `
      -PassThru
  }
  finally {
    Restore-LaunchEnvironment $launchEnvironment
  }

  Start-Sleep -Seconds 2
  if ($process.HasExited) {
    if (Test-Path -LiteralPath $desktopOutLog) {
      Get-Content -LiteralPath $desktopOutLog -Tail 120
    }
    if (Test-Path -LiteralPath $desktopErrLog) {
      Get-Content -LiteralPath $desktopErrLog -Tail 120
    }
    throw "Native WebView shell exited early with code $($process.ExitCode)"
  }

  Write-Info "Native WebView shell PID: $($process.Id)"
  Write-Info "Logs: $desktopOutLog and $desktopErrLog"
}

try {
  $protectedProcessIds = Get-CurrentProcessAncestorIds

  if ($Status) {
    Show-Status
    exit 0
  }

  Stop-ProjectProcesses
  Invoke-HelperServicePrepackageCleanup

  if (-not $Quick) {
    Clean-BuildArtifacts
  }
  else {
    Write-Info 'Quick mode enabled; skipping artifact cleanup'
  }

  if ($CleanOnly) {
    Write-Info 'Clean-only mode complete'
    exit 0
  }

  Build-WebViewPackage
  Test-WebViewPackage
  Show-Status

  if ($PackageDir -or $Package) {
    Write-Info "WebView package ready at $packageRoot"
    exit 0
  }

  if (-not $NoLaunch) {
    Start-WebViewShell
  }
  else {
    Write-Info 'Build completed without launching native WebView shell'
  }
}
catch {
  Write-Error $_
  if ($PauseOnError) {
    Read-Host 'Press Enter to close this elevated window'
  }
  exit 1
}
