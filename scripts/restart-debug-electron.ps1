param(
  [switch]$SkipClean,
  [switch]$NoFrontendBuild,
  [switch]$BuildAllBackend,
  [string[]]$BackendTargets = @('exv', 'exv-helper'),
  [int]$FrontendPort = 5176
)

$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir
$webuiDir = Join-Path $repoRoot 'webui'
$cppBuildDir = Join-Path $repoRoot 'build-windows'
$distDir = Join-Path $webuiDir 'dist'
$distElectronDir = Join-Path $webuiDir 'dist-electron'
$repoMarker = $repoRoot
$electronLogOut = Join-Path $webuiDir 'debug-electron.out.log'
$electronLogErr = Join-Path $webuiDir 'debug-electron.err.log'
$electronWaitSeconds = 120

function Write-Info([string]$Message) {
  Write-Output "[INFO] $Message"
}

if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
  throw 'pnpm is required for this repository. Install pnpm before running restart-debug-electron.ps1.'
}
$packageManager = 'pnpm'
$packageManagerPath = (Get-Command $packageManager -ErrorAction Stop).Source

function Invoke-Step {
  param(
    [Parameter(Mandatory = $true)][string]$Command,
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments
  )

  & $Command @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "Command failed: $Command $($Arguments -join ' ')"
  }
}

function Get-ProcessesOnPort {
  param([int]$Port)

  $holders = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue |
    Select-Object -ExpandProperty OwningProcess -Unique
  if (-not $holders) {
    return @()
  }

  return Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessId -in $holders }
}

function Stop-ProcessSafe {
  param([int[]]$ProcessIds)

  foreach ($id in $ProcessIds | Sort-Object -Unique) {
    if (-not (Get-Process -Id $id -ErrorAction SilentlyContinue)) {
      continue
    }

    Write-Info "Stopping process $id ..."
    try {
      Stop-Process -Id $id -ErrorAction Stop
      continue
    }
    catch {
      try {
        taskkill /PID $id /F /T | Out-Null
      }
      catch {
        Write-Info "  failed to stop ${id} via taskkill: $($_.Exception.Message)"
      }
      try {
        $cimProc = Get-CimInstance Win32_Process -Filter "ProcessId=$id" -ErrorAction SilentlyContinue
        if ($cimProc) {
          $result = $cimProc | Invoke-CimMethod -MethodName Terminate -ErrorAction Stop
          if ($result.ReturnValue -ne 0) {
            Write-Info "  CIM terminate returned code $($result.ReturnValue) for $id."
          }
        }
      }
      catch {
        Write-Info "  failed to stop ${id} via CIM: $($_.Exception.Message)"
      }
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

  Write-Info "Stopping service $serviceName ..."
  try {
    Stop-Service -Name $serviceName -ErrorAction Stop -Force
  }
  catch {
    try {
      sc.exe stop $serviceName | Out-Null
    }
    catch {
      Write-Info "  failed to stop service ${serviceName}: $($_.Exception.Message)"
    }
  }

  $deadline = (Get-Date).AddSeconds(8)
  while ((Get-Date) -lt $deadline) {
    $svc = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if (-not $svc -or $svc.Status -ne 'Running') {
      return
    }
    Start-Sleep -Milliseconds 200
  }

  Write-Info "  service $serviceName still running."
}

function Get-DebugProcesses {
  $nameSet = @(
    'node.exe',
    'electron.exe',
    'pnpm.exe',
    'exv-helper.exe',
    'exv.exe',
    'cmd.exe',
    'powershell.exe',
    'pwsh.exe'
  )

  $commandPatterns = @(
    "*$repoMarker*webui*",
    '*desktop:dev*',
    '*dist-electron*main*index.js*',
    '*wait-on http://localhost:5176*',
    '*vite --port*5176*',
    '*electron:start*',
    '*cross-env*dist-electron*main*index.js*',
    '*electron\\cli.js*dist-electron*main*index.js*',
    '*pnpm run*desktop:dev*',
    '*exv-helper.exe*',
    '*exv.exe*'
  )

  $portListeners = Get-ProcessesOnPort -Port $FrontendPort
  $results = @()
  if ($portListeners) {
    $results += $portListeners
  }

  $procs = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue
  foreach ($proc in $procs) {
    if ($nameSet -notcontains $proc.Name) {
      continue
    }

    if ([string]::IsNullOrWhiteSpace($proc.CommandLine)) {
      $results += $proc
      continue
    }

    foreach ($pattern in $commandPatterns) {
      if ($proc.CommandLine -like $pattern) {
        $results += $proc
        break
      }
    }
  }

  return ($results | Sort-Object ProcessId -Unique)
}

function Stop-DebugProcesses {
  Write-Info 'Terminating existing ECNU-VPN debug processes...'
  Stop-HelperService
  $candidates = Get-DebugProcesses
  if (-not $candidates) {
    Write-Info 'No matched debug processes found.'
    return
  }

  foreach ($candidate in $candidates) {
    Write-Info "  stop $($candidate.Name) [PID=$($candidate.ProcessId)]"
    Stop-ProcessSafe -ProcessIds @($candidate.ProcessId)
  }

  $deadline = (Get-Date).AddSeconds(3)
  while ((Get-Date) -lt $deadline) {
    $remain = Get-DebugProcesses
    if (-not $remain) {
      break
    }
    Start-Sleep -Milliseconds 500
  }
}

function Ensure-FrontendPortFree {
  $holders = Get-ProcessesOnPort -Port $FrontendPort
  if (-not $holders) {
    return
  }

  Write-Info "Port $FrontendPort is occupied; releasing holders."
  foreach ($holder in $holders) {
    Write-Info "  holder PID=$($holder.ProcessId) Name=$($holder.Name) Cmd=$($holder.CommandLine)"
  }
  Stop-ProcessSafe -ProcessIds ($holders | Select-Object -ExpandProperty ProcessId -Unique)
  Start-Sleep -Milliseconds 500
}

function Remove-DirectorySafe {
  param([Parameter(Mandatory = $true)][string]$Path)
  if (-not (Test-Path $Path)) {
    return
  }

  Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
}

function Configure-Backend {
  Write-Info "Configuring backend ($env:USERNAME)..."
  if (-not $SkipClean -and (Test-Path $cppBuildDir)) {
    Remove-DirectorySafe $cppBuildDir
  }

  Push-Location $repoRoot
  try {
    Invoke-Step cmake --preset windows-release
  }
  finally {
    Pop-Location
  }
}

function Build-Backend {
  Write-Info "Building backend ($env:USERNAME)..."
  $artifactFiles = @()
  if ($BuildAllBackend -or $BackendTargets -contains 'exv') {
    $artifactFiles += (Join-Path $cppBuildDir 'cpp/exv.exe')
    $artifactFiles += (Join-Path $repoRoot 'build/windows/electron/bin/exv.exe')
  }
  if ($BuildAllBackend -or $BackendTargets -contains 'exv-helper') {
    $artifactFiles += (Join-Path $cppBuildDir 'cpp/exv-helper.exe')
    $artifactFiles += (Join-Path $repoRoot 'build/windows/electron/bin/exv-helper.exe')
  }
  Ensure-ArtifactsUnlocked $artifactFiles
  Push-Location $repoRoot
  try {
    if ($BuildAllBackend) {
      Invoke-Step cmake --build --preset windows-release
    }
    else {
      $buildTargets = @()
      foreach ($t in $BackendTargets) {
        if ([string]::IsNullOrWhiteSpace($t)) {
          continue
        }
        $buildTargets += '--target'
        $buildTargets += $t
      }
      if ($buildTargets.Count -eq 0) {
        Invoke-Step cmake --build --preset windows-release
      }
      else {
        Invoke-Step cmake --build --preset windows-release @buildTargets
      }
    }
  }
  finally {
    Pop-Location
  }
}

function Ensure-ArtifactsUnlocked {
  param([string[]]$Files)
  foreach ($file in $Files) {
    if (-not (Test-Path $file)) {
      continue
    }

    $retries = 20
    for ($i = 1; $i -le $retries; $i++) {
      try {
        $stream = [System.IO.File]::OpenWrite($file)
        $stream.Close()
        break
      }
      catch {
        if ($i -eq $retries) {
          Write-Info "File is still locked, retrying process cleanup for $file"
          Stop-DebugProcesses
          Start-Sleep -Milliseconds 500
          $service = Get-Service -Name 'exv-helper' -ErrorAction SilentlyContinue
          if ($service -and $service.Status -eq 'Running') {
            throw 'Cannot rebuild exv-helper: service exv-helper is still running (likely requires administrator to stop). Please run this script from an elevated PowerShell window or stop the service first.'
          }
          throw "Cannot unlock $file for build (permission denied)."
        }
        Start-Sleep -Milliseconds 250
      }
    }
  }
}

function Build-WebUI {
  if ($NoFrontendBuild) {
    Write-Info 'Skipping webui rebuild per request.'
    return
  }

  Write-Info 'Rebuilding web UI...'
  if (-not $SkipClean) {
    Remove-DirectorySafe $distDir
    Remove-DirectorySafe $distElectronDir
  }

  Push-Location $webuiDir
  try {
    if (Test-Path (Join-Path $webuiDir 'pnpm-lock.yaml')) {
      Invoke-Step pnpm install --dir . --frozen-lockfile --prefer-offline
    }
    else {
      Invoke-Step pnpm install --dir .
    }
    Invoke-Step pnpm run build
    Invoke-Step pnpm run build:electron
    Ensure-ElectronBinary
  }
  finally {
    Pop-Location
  }
}

function Ensure-ElectronBinary {
  $electronExecutable = Join-Path $webuiDir 'node_modules/electron/dist/electron.exe'
  if (Test-Path $electronExecutable) {
    return
  }

  $installScript = Join-Path $webuiDir 'node_modules/electron/install.js'
  if (-not (Test-Path $installScript)) {
    Write-Info 'electron install script not found; attempting npm rebuild fallback.'
    Invoke-Step pnpm rebuild electron
    return
  }

  Write-Info 'Electron binary missing. Running electron install script to repair.'
  Invoke-Step node $installScript
}

function Find-ElectronProcess {
  $commandPatterns = @(
    "*$repoRoot*dist-electron*main*index.js*",
    "*dist-electron*main*index.js*",
    "*electron*dist-electron*main*index.js*",
    "*electron\\cli.js*dist-electron*main*index.js*",
    "*node_modules\\electron\\cli.js*dist-electron*main*index.js*",
    "*node_modules/.pnpm*electron*cli.js*dist-electron*main*index.js*"
  )

  $process = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object {
      $commandLine = $_.CommandLine
      $_.CommandLine -and
      (
        $_.Name -eq 'electron.exe' -or
        $_.Name -eq 'node.exe'
      ) -and
      ($commandPatterns | ForEach-Object { $commandLine -like $_ } | Where-Object { $_ } )
    } |
    Select-Object -First 1

  return $process
}

function Start-ElectronDebug {
  Write-Info 'Starting Electron debug instance...'
  if (Test-Path $electronLogOut) { Remove-Item -Force $electronLogOut -ErrorAction SilentlyContinue }
  if (Test-Path $electronLogErr) { Remove-Item -Force $electronLogErr -ErrorAction SilentlyContinue }

  $runnerArgs = @('run', 'desktop:dev')
  Write-Info "Launch command: $packageManager $($runnerArgs -join ' ') (cwd=$webuiDir)"

  $launcherFile = $packageManagerPath
  $launcherArgs = $runnerArgs
  if ([System.IO.Path]::GetExtension($packageManagerPath) -ieq '.ps1') {
    if (Get-Command pwsh -ErrorAction SilentlyContinue) {
      $launcherFile = (Get-Command pwsh).Source
    }
    else {
      $launcherFile = (Get-Command powershell).Source
    }
    $quotedScript = '"' + $packageManagerPath.Replace('"', '\"') + '"'
    $joinedArgs = $runnerArgs -join ' '
    $launcherArgs = @('-NoProfile', '-NoLogo', '-ExecutionPolicy', 'Bypass', '-Command', "& $quotedScript $joinedArgs")
  }

  $p = Start-Process -FilePath $launcherFile `
    -ArgumentList $launcherArgs `
    -WorkingDirectory $webuiDir `
    -RedirectStandardOutput $electronLogOut `
    -RedirectStandardError $electronLogErr `
    -WindowStyle Hidden `
    -PassThru

  Write-Info "Launcher PID: $($p.Id)"

  Start-Sleep -Milliseconds 500
  if ($p.HasExited) {
    if (Test-Path $electronLogOut) {
      Write-Info '[stdout tail]'
      Get-Content $electronLogOut -Tail 120 | ForEach-Object { Write-Host $_ }
    }
    if (Test-Path $electronLogErr) {
      Write-Info '[stderr tail]'
      Get-Content $electronLogErr -Tail 120 | ForEach-Object { Write-Host $_ }
    }
    throw "Launcher exited with code $($p.ExitCode)"
  }

  $timeout = $electronWaitSeconds
  $waited = 0.0
  $portProc = $null
  $electronPid = $null

  do {
    Start-Sleep -Milliseconds 500
    $waited += 0.5

    $portProc = (Get-ProcessesOnPort -Port $FrontendPort | Select-Object -First 1 -ExpandProperty ProcessId)
    $electronProc = Find-ElectronProcess
    if ($electronProc) {
      $electronPid = $electronProc.ProcessId
    }

    if ($portProc -and $electronPid) {
      break
    }

    if ($p.HasExited) {
      break
    }
  } while ($waited -lt $timeout)

  if ($portProc) {
    Write-Info "Frontend dev server listening on :$FrontendPort (PID $portProc)"
  }
  else {
    Write-Info "Frontend dev server on :$FrontendPort not observed yet."
  }

  if ($electronPid) {
    Write-Info "Electron process detected (PID $electronPid)."
  }
  else {
    Write-Info 'Electron process not detected yet; check logs.'
  }

  if (Test-Path $electronLogOut) {
    Write-Info '[stdout tail]'
    Get-Content $electronLogOut -Tail 40 | ForEach-Object { Write-Host $_ }
  }
  if (Test-Path $electronLogErr) {
    Write-Info '[stderr tail]'
    Get-Content $electronLogErr -Tail 40 | ForEach-Object { Write-Host $_ }
  }

  if (-not $portProc) {
    throw "Frontend dev server didn't open port $FrontendPort in time."
  }
  if (-not $electronPid) {
    throw 'Electron process not detected while dev server is available.'
  }

  return @($p.Id, $electronPid, $portProc)
}

Stop-DebugProcesses
Ensure-FrontendPortFree
Configure-Backend
Build-WebUI
Build-Backend

$result = Start-ElectronDebug
$launcherPid = $result[0]
$electronPid = $result[1]
$vitePid = $result[2]

Write-Info "Done. Use Ctrl+C in this script window to stop or manually kill:"
Write-Output "- Launcher PID: $launcherPid"
Write-Output "- Electron PID: $electronPid"
Write-Output "- Vite listen port: $FrontendPort (PID $vitePid)"
