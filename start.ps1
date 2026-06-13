param(
  [switch]$Quick,
  [switch]$NoFrontendBuild,
  [switch]$NoLaunch,
  [switch]$PackageDir,
  [switch]$Package,
  [switch]$CleanOnly,
  [switch]$Status,
  [int]$Port = 8288
)

$ErrorActionPreference = 'Stop'

# ── Auto-elevation via UAC ──────────────────────────────────────────────
# When not running as Administrator, re-launch the script with RunAs (UAC).
# The user authorises the prompt; the elevated instance inherits all arguments.
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

  Write-Output "[start] Not running as Administrator — requesting elevation via UAC..."
  $elevated = Start-Process -FilePath 'powershell.exe' `
    -ArgumentList "-NoProfile -NoLogo -ExecutionPolicy Bypass -File `"$scriptPath`"$argString" `
    -Verb RunAs -Wait -PassThru
  exit $elevated.ExitCode
}
# ────────────────────────────────────────────────────────────────────────

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$webuiDir = Join-Path $repoRoot 'webui'
$cppBuildRoot = Join-Path $repoRoot 'build-windows'
$cppBuildDir = Join-Path $cppBuildRoot 'cpp'
$electronRoot = Join-Path $repoRoot 'build\windows\electron'
$electronNativeBinDir = Join-Path $electronRoot 'native\bin'
$compatElectronBinDir = Join-Path $electronRoot 'bin'
$rendererDistDir = Join-Path $electronRoot 'dist'
$electronDistDir = Join-Path $electronRoot 'dist-electron'
$electronReleaseDir = Join-Path $electronRoot 'release'
$legacyWebuiNativeBinDir = Join-Path $webuiDir 'native\bin'
$desktopOutLog = Join-Path $webuiDir 'start-desktop.out.log'
$desktopErrLog = Join-Path $webuiDir 'start-desktop.err.log'
$protectedProcessIds = @()

function Write-Info([string]$Message) {
  Write-Output "[start] $Message"
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
    if (-not $seen.Add($current)) {
      break
    }
    $ids += $current
    if (-not $parents.ContainsKey($current)) {
      break
    }
    $parent = $parents[$current]
    if ($parent -le 0) {
      break
    }
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

function Get-ProjectDebugProcesses {
  $results = @()
  $alwaysMatchNames = @('exv.exe', 'exv-helper.exe')
  $conditionalNames = @('node.exe', 'electron.exe', 'pnpm.exe', 'cmd.exe', 'powershell.exe', 'pwsh.exe')
  $commandPatterns = @(
    "*$repoRoot*",
    '*desktop:dev*',
    '*electron:start*',
    '*dist-electron*main*index.js*',
    "*localhost:$Port*",
    "*--port $Port*",
    '*exv-helper.exe*',
    '*exv.exe*'
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
  $serviceName = 'exv-helper'
  $service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
  if (-not $service) {
    return
  }

  if ($service.Status -eq 'Stopped') {
    return
  }

  Write-Info "Stopping service $serviceName"
  try {
    Stop-Service -Name $serviceName -Force -ErrorAction Stop
  }
  catch {
    try {
      sc.exe stop $serviceName | Out-Null
    }
    catch {
      Write-Info "Failed to stop service ${serviceName}: $($_.Exception.Message)"
    }
  }

  $deadline = (Get-Date).AddSeconds(8)
  while ((Get-Date) -lt $deadline) {
    $current = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if (-not $current -or $current.Status -ne 'Running') {
      return
    }
    Start-Sleep -Milliseconds 250
  }
}

function Stop-DebugProcesses {
  Write-Info 'Stopping residual debug processes'
  Stop-HelperService

  foreach ($candidate in Get-ProjectDebugProcesses) {
    Write-Info "Stopping $($candidate.Name)[$($candidate.ProcessId)]"
    Stop-ProcessTreeSafe -ProcessId ([int]$candidate.ProcessId) -Name $candidate.Name
  }

  $deadline = (Get-Date).AddSeconds(5)
  while ((Get-Date) -lt $deadline) {
    $remaining = Get-ProjectDebugProcesses
    if (-not $remaining) {
      return
    }
    Start-Sleep -Milliseconds 300
  }

  $remaining = Get-ProjectDebugProcesses
  if ($remaining) {
    $summary = ($remaining | ForEach-Object { "$($_.Name)[$($_.ProcessId)]" }) -join ', '
    throw "Residual debug processes still running: $summary. Re-run from an elevated PowerShell terminal so start.ps1 can terminate elevated helper/service processes before rebuilding."
  }
}

function Remove-DirectorySafe {
  param([Parameter(Mandatory = $true)][string]$Path)

  if (-not (Test-Path -LiteralPath $Path)) {
    return
  }

  Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
}

function Ensure-Pnpm {
  $command = Get-Command pnpm -ErrorAction SilentlyContinue
  if (-not $command) {
    throw 'pnpm is required for this repository. Install pnpm before running start.ps1.'
  }
  return $command.Source
}

function Ensure-PnpmInstall {
  $nodeModulesDir = Join-Path $webuiDir 'node_modules'
  if ($Quick -and (Test-Path -LiteralPath $nodeModulesDir)) {
    return
  }

  $args = @('install')
  if (Test-Path -LiteralPath (Join-Path $webuiDir 'pnpm-lock.yaml')) {
    $args += '--frozen-lockfile'
  }
  Invoke-Step -Command (Ensure-Pnpm) -Arguments $args -WorkingDirectory $webuiDir
}

function Clean-BuildArtifacts {
  Write-Info 'Removing build and staging artifacts'
  Remove-DirectorySafe $cppBuildRoot
  Remove-DirectorySafe $compatElectronBinDir
  Remove-DirectorySafe $electronNativeBinDir
  Remove-DirectorySafe $electronReleaseDir
  Remove-DirectorySafe $legacyWebuiNativeBinDir
  Remove-DirectorySafe $rendererDistDir
  Remove-DirectorySafe $electronDistDir
  if (Test-Path -LiteralPath $desktopOutLog) { Remove-Item -LiteralPath $desktopOutLog -Force -ErrorAction SilentlyContinue }
  if (Test-Path -LiteralPath $desktopErrLog) { Remove-Item -LiteralPath $desktopErrLog -Force -ErrorAction SilentlyContinue }
}

function Build-Backend {
  Write-Info 'Running: cmake --preset windows-release'
  Invoke-Step -Command 'cmake' -Arguments @('--preset', 'windows-release')

  Write-Info 'Running: cmake --build --preset windows-release --target exv exv-helper backend_resolver_test win32_helper_oneshot_test'
  Invoke-Step -Command 'cmake' -Arguments @(
    '--build', '--preset', 'windows-release',
    '--target', 'exv', 'exv-helper', 'backend_resolver_test', 'win32_helper_oneshot_test'
  )
}

function Build-FrontendArtifacts {
  Ensure-PnpmInstall

  if (-not $NoFrontendBuild) {
    Write-Info 'Running: pnpm run build'
    Invoke-Step -Command (Ensure-Pnpm) -Arguments @('run', 'build') -WorkingDirectory $webuiDir

    Write-Info 'Running: pnpm run build:electron'
    Invoke-Step -Command (Ensure-Pnpm) -Arguments @('run', 'build:electron') -WorkingDirectory $webuiDir
  }
  else {
    Write-Info 'Skipping frontend bundle rebuild because -NoFrontendBuild was provided'
  }

  Write-Info 'Running: pnpm run prepare:native'
  Invoke-Step -Command (Ensure-Pnpm) -Arguments @('run', 'prepare:native') -WorkingDirectory $webuiDir
}

function Copy-CompatibilityBinaries {
  if (-not (Test-Path -LiteralPath $electronNativeBinDir)) {
    throw "Native staging directory is missing: $electronNativeBinDir"
  }

  Remove-DirectorySafe $compatElectronBinDir
  New-Item -ItemType Directory -Path $compatElectronBinDir -Force | Out-Null

  Get-ChildItem -LiteralPath $electronNativeBinDir -File | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $compatElectronBinDir $_.Name) -Force
  }
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
  Write-Info 'Backend build outputs'
  Show-FileHashIfPresent (Join-Path $cppBuildDir 'exv.exe')
  Show-FileHashIfPresent (Join-Path $cppBuildDir 'exv-helper.exe')

  Write-Info 'Electron native staging outputs'
  Show-FileHashIfPresent (Join-Path $electronNativeBinDir 'exv.exe')
  Show-FileHashIfPresent (Join-Path $electronNativeBinDir 'exv-helper.exe')

  Write-Info 'Compatibility outputs'
  Show-FileHashIfPresent (Join-Path $compatElectronBinDir 'exv.exe')
  Show-FileHashIfPresent (Join-Path $compatElectronBinDir 'exv-helper.exe')

  Invoke-DesktopRpcProbe (Join-Path $electronNativeBinDir 'exv.exe')
  Invoke-DesktopRpcProbe (Join-Path $compatElectronBinDir 'exv.exe')
}

function Find-ElectronProcess {
  return Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object {
      $cmd = [string]$_.CommandLine
      $_.Name -in @('electron.exe', 'node.exe') -and
      -not [string]::IsNullOrWhiteSpace($cmd) -and
      $cmd -like '*dist-electron*main*index.js*'
    } |
    Select-Object -First 1
}

function Start-DesktopDev {
  $pnpm = Ensure-Pnpm
  $runnerArgs = @('run', 'desktop:dev')
  $launcherFile = $pnpm
  $launcherArgs = $runnerArgs

  if ([System.IO.Path]::GetExtension($pnpm) -ieq '.ps1') {
    $shell = if (Get-Command pwsh -ErrorAction SilentlyContinue) {
      (Get-Command pwsh).Source
    }
    else {
      (Get-Command powershell).Source
    }
    $escapedPnpm = $pnpm.Replace("'", "''")
    $launcherFile = $shell
    $launcherArgs = @('-NoProfile', '-NoLogo', '-ExecutionPolicy', 'Bypass', '-Command', "& '$escapedPnpm' run desktop:dev")
  }

  if (Test-Path -LiteralPath $desktopOutLog) { Remove-Item -LiteralPath $desktopOutLog -Force -ErrorAction SilentlyContinue }
  if (Test-Path -LiteralPath $desktopErrLog) { Remove-Item -LiteralPath $desktopErrLog -Force -ErrorAction SilentlyContinue }

  Write-Info 'Launching desktop dev workflow'
  $process = Start-Process -FilePath $launcherFile `
    -ArgumentList $launcherArgs `
    -WorkingDirectory $webuiDir `
    -RedirectStandardOutput $desktopOutLog `
    -RedirectStandardError $desktopErrLog `
    -PassThru

  Start-Sleep -Seconds 2
  if ($process.HasExited) {
    if (Test-Path -LiteralPath $desktopOutLog) {
      Get-Content -LiteralPath $desktopOutLog -Tail 120
    }
    if (Test-Path -LiteralPath $desktopErrLog) {
      Get-Content -LiteralPath $desktopErrLog -Tail 120
    }
    throw "desktop:dev launcher exited early with code $($process.ExitCode)"
  }

  $electron = $null
  $deadline = (Get-Date).AddSeconds(90)
  while ((Get-Date) -lt $deadline) {
    $listener = Get-ProcessesOnPort -LocalPort $Port
    $electron = Find-ElectronProcess
    if ($listener -and $electron) {
      break
    }
    if ($process.HasExited) {
      break
    }
    Start-Sleep -Milliseconds 500
  }

  if (-not (Get-ProcessesOnPort -LocalPort $Port)) {
    throw "Frontend dev server did not open port $Port in time. Check $desktopOutLog and $desktopErrLog."
  }

  if (-not $electron) {
    throw "Electron process was not detected in time. Check $desktopOutLog and $desktopErrLog."
  }

  Write-Info "Desktop dev launcher PID: $($process.Id)"
  Write-Info "Electron PID: $($electron.ProcessId)"
  Write-Info "Logs: $desktopOutLog and $desktopErrLog"
}

$protectedProcessIds = Get-CurrentProcessAncestorIds

if ($Status) {
  Show-Status
  exit 0
}

Stop-DebugProcesses

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

Build-Backend
Build-FrontendArtifacts
Copy-CompatibilityBinaries

if ($PackageDir) {
  Write-Info 'Running: pnpm run desktop:package:dir'
  Invoke-Step -Command (Ensure-Pnpm) -Arguments @('run', 'desktop:package:dir') -WorkingDirectory $webuiDir
}

if ($Package) {
  Write-Info 'Running: pnpm run desktop:package'
  Invoke-Step -Command (Ensure-Pnpm) -Arguments @('run', 'desktop:package') -WorkingDirectory $webuiDir
}

Show-Status

if (-not $NoLaunch -and -not $PackageDir -and -not $Package) {
  Start-DesktopDev
}
else {
  Write-Info 'Build completed without launching desktop dev workflow'
}
