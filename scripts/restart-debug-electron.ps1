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

function Write-Info([string]$Message) {
  Write-Output "[INFO] $Message"
}

$packageManager = if (Get-Command pnpm -ErrorAction SilentlyContinue) { 'pnpm' } else { 'npm' }
if ($packageManager -eq 'npm') {
  Write-Info 'pnpm not found, using npm fallback.'
}

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

function Format-Arg([string]$Arg) {
  if ($Arg -match '[\s"]') {
    return '"' + $Arg.Replace('"', '\"') + '"'
  }
  return $Arg
}

function Stop-ProcessSafe {
  param([int[]]$ProcessIds)
  foreach ($id in $ProcessIds) {
    try {
      Stop-Process -Id $id -Force -ErrorAction Stop
    }
    catch {
      Write-Info "Failed to stop process ${id}: $($_.Exception.Message)"
    }
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

function Stop-DebugProcesses {
  Write-Info 'Terminating existing ECNU-VPN debug processes...'

  $names = @(
    'node.exe',
    'electron.exe',
    'conhost.exe',
    'exv-helper.exe',
    'exv.exe'
  )
  $patterns = @(
    "*$repoMarker*webui*",
    '*pnpm run desktop:dev*',
    '*npm run desktop:dev*',
    '*pnpm --dir *desktop:dev*',
    '*npm --prefix *desktop:dev*',
    '*dist-electron*main*index.js*',
    '*exv-helper.exe*',
    '*exv.exe*'
  )

  $candidates = @()
  $listeners = Get-ProcessesOnPort -Port $FrontendPort
  if ($listeners) {
    $candidates += $listeners
  }

  foreach ($name in $names) {
    $procsByName = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -eq $name -and $_.CommandLine }

    foreach ($proc in $procsByName) {
      $isMatch = $false
      foreach ($pattern in $patterns) {
        if ($proc.CommandLine -like $pattern) {
          $isMatch = $true
          break
        }
      }
      if ($isMatch) {
        $candidates += $proc
      }
    }
  }

  $candidates = $candidates | Sort-Object ProcessId -Unique
  if (-not $candidates) {
    Write-Info 'No matched debug processes found.'
  }
  else {
    $candidates | ForEach-Object {
      Write-Info "Stop process $($_.Name) [PID=$($_.ProcessId)]"
      Stop-ProcessSafe -ProcessIds @($_.ProcessId)
    }
  }

  Start-Sleep -Milliseconds 500
}

function Ensure-FrontendPortFree {
  $holders = Get-ProcessesOnPort -Port $FrontendPort
  if (-not $holders) {
    return
  }

  Write-Info "Port $FrontendPort is occupied; releasing holders."
  $holders | ForEach-Object {
    Write-Info "  Holder PID=$($_.ProcessId) Name=$($_.Name) Cmd=$($_.CommandLine)"
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

function Build-Backend {
  Write-Info "Rebuilding backend ($env:USERNAME)..."
  if (-not $SkipClean -and (Test-Path $cppBuildDir)) {
    Remove-DirectorySafe $cppBuildDir
  }
  Push-Location $repoRoot
  try {
    Invoke-Step cmake --preset windows-release
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
    if ($packageManager -eq 'pnpm') {
      if (Test-Path (Join-Path $webuiDir 'pnpm-lock.yaml')) {
        Invoke-Step pnpm install --dir . --frozen-lockfile --prefer-offline
      }
      else {
        Invoke-Step pnpm install --dir .
      }
      Invoke-Step pnpm run build
      Invoke-Step pnpm run build:electron
    }
    else {
      Invoke-Step npm ci
      Invoke-Step npm run build
      Invoke-Step npm run build:electron
    }
  }
  finally {
    Pop-Location
  }
}

function Start-ElectronDebug {
  Write-Info 'Starting Electron debug instance...'

  if (Test-Path $electronLogOut) { Remove-Item -Force $electronLogOut -ErrorAction SilentlyContinue }
  if (Test-Path $electronLogErr) { Remove-Item -Force $electronLogErr -ErrorAction SilentlyContinue }

  if ($packageManager -eq 'pnpm') {
    $runnerArgs = @('run', 'desktop:dev')
  }
  else {
    $runnerArgs = @('run', 'desktop:dev')
  }

  Write-Info "Launch command: $packageManager $($runnerArgs -join ' ')"
  $commandLineParts = @($packageManager) + $runnerArgs
  $commandLine = ($commandLineParts | ForEach-Object { Format-Arg $_ }) -join ' '
  $commandLine = "cd /d `"$webuiDir`" && $commandLine"
  Write-Info "Launcher commandline: $commandLine"
  $p = Start-Process -FilePath $env:ComSpec `
    -ArgumentList '/d', '/c', $commandLine `
    -RedirectStandardOutput $electronLogOut `
    -RedirectStandardError $electronLogErr `
    -WindowStyle Hidden `
    -PassThru

  Write-Info "Launcher PID: $($p.Id)"
  if ($p.HasExited -and $LASTEXITCODE -ne 0) {
    throw "Launcher exited with code $LASTEXITCODE"
  }

  $timeout = 120
  $waited = 0
  $portProc = $null
  $electronPid = $null
  do {
    Start-Sleep -Milliseconds 500
    $waited += 0.5

    $portProc = (Get-ProcessesOnPort -Port $FrontendPort | Select-Object -First 1 -ExpandProperty ProcessId)
    $electronProc = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
      Where-Object {
        $_.Name -eq 'electron.exe' -and
        $_.CommandLine -and
        $_.CommandLine -like "*$repoMarker*dist-electron*main*index.js*"
      } | Select-Object -First 1
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
    Write-Info 'Electron process not detected yet; check logs if this persists.'
    if (Test-Path $electronLogOut) {
      Write-Info '[stdout tail]'
      Get-Content $electronLogOut -Tail 120 | ForEach-Object { Write-Host $_ }
    }
    if (Test-Path $electronLogErr) {
      Write-Info '[stderr tail]'
      Get-Content $electronLogErr -Tail 120 | ForEach-Object { Write-Host $_ }
    }
  }

  if (-not $portProc -or -not $electronPid) {
    if (-not (Get-Process -Id $p.Id -ErrorAction SilentlyContinue)) {
      throw 'Launcher exited before Electron became available. Check debug-electron logs.'
    }
    if (-not $portProc) {
      throw "Frontend dev server didn't open port $FrontendPort in time."
    }
    if (-not $electronPid) {
      throw 'Electron process not detected while dev server is available.'
    }
  }

  return $p.Id
}

Stop-DebugProcesses
Ensure-FrontendPortFree
Build-Backend
Build-WebUI

$launcherPid = Start-ElectronDebug

Write-Info "Done. Use Ctrl+C to stop this window or manually kill:"
Write-Output "- Launcher PID: $launcherPid"
Write-Output "- Vite listen port: $FrontendPort"
