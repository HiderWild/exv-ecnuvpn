param(
  [string]$PackageRoot = "",
  [string]$OutputRoot = "",
  [string]$StateDir = "",
  [int]$MonitorSeconds = 90,
  [int]$SampleIntervalMs = 500,
  [switch]$NoLaunch,
  [switch]$IncludeRawLogDelta,
  [switch]$CaptureScreenshot
)

$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir

if (-not $PackageRoot) {
  $PackageRoot = Join-Path $repoRoot 'build\windows\webview\package\ECNU VPN'
}
if (-not $OutputRoot) {
  $OutputRoot = Join-Path $repoRoot 'build\manual-verification'
}
if ($StateDir) {
  $env:ECNUVPN_STATE_DIR = $StateDir
}

function Resolve-StateDir {
  if ($env:ECNUVPN_STATE_DIR) {
    return $env:ECNUVPN_STATE_DIR
  }
  if ($env:APPDATA) {
    return Join-Path $env:APPDATA 'ecnuvpn'
  }
  return Join-Path $env:ProgramData 'ecnuvpn'
}

function Redact-LogLine {
  param([string]$Line)

  $redacted = $Line
  $patterns = @(
    '(?i)(password|passwd|token|secret|credential|auth_token|session_cookie|webvpn_cookie|csrf_token|bearer_token|api_key|apikey)=\S+',
    '(?i)("?(password|passwd|token|secret|credential|auth_token|session_cookie|webvpn_cookie|csrf_token|bearer_token|api_key|apikey)"?\s*[:=]\s*)("[^"]*"|\S+)',
    '(?i)(Cookie:\s*)\S+',
    '(?i)(Authorization:\s*)\S+'
  )
  foreach ($pattern in $patterns) {
    $redacted = [regex]::Replace($redacted, $pattern, '$1<redacted>')
  }
  return $redacted
}

function Read-TextFromOffset {
  param(
    [string]$Path,
    [long]$Offset
  )

  if (-not (Test-Path -LiteralPath $Path)) {
    return "Log file was not present: $Path"
  }

  $stream = [System.IO.File]::Open(
    $Path,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read,
    [System.IO.FileShare]::ReadWrite)
  try {
    if ($Offset -lt 0 -or $Offset -gt $stream.Length) {
      $Offset = 0
    }
    [void]$stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
    $reader = New-Object System.IO.StreamReader($stream, [System.Text.Encoding]::UTF8, $true)
    try {
      $content = $reader.ReadToEnd()
    }
    finally {
      $reader.Dispose()
    }
  }
  finally {
    $stream.Dispose()
  }

  if (-not $content) {
    return "No new log content after offset $Offset."
  }
  return $content
}

function Write-LogEvidence {
  param(
    [string]$Path,
    [long]$Offset,
    [string]$SummaryPath,
    [string]$RawPath,
    [bool]$IncludeRaw
  )

  $content = Read-TextFromOffset -Path $Path -Offset $Offset
  $lines = $content -split "`r?`n"
  $summaryLines = @()
  foreach ($line in $lines) {
    if ($line -match '\[connect-timing\]' -or
        $line -match 'app_api:' -or
        $line -match 'Helper connector:' -or
        $line -match 'Native engine session' -or
        $line -match 'Connect requested' -or
        $line -match 'Helper session started') {
      $summaryLines += (Redact-LogLine -Line $line)
    }
  }

  if ($summaryLines.Count -eq 0) {
    $summaryLines = @('No connect-stage summary lines were found in the current-session log delta.')
  }
  $summaryLines | Set-Content -LiteralPath $SummaryPath -Encoding UTF8

  if ($IncludeRaw) {
    $rawLines = @()
    foreach ($line in $lines) {
      $rawLines += (Redact-LogLine -Line $line)
    }
    $rawLines | Set-Content -LiteralPath $RawPath -Encoding UTF8
  }
}

function Write-ProcessSnapshot {
  param([string]$OutPath)

  $rows = @()
  $processes = Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessName -in @('exv-ui', 'exv', 'exv-helper') }
  foreach ($process in $processes) {
    $startTime = ''
    try {
      $startTime = $process.StartTime.ToString('o')
    }
    catch {
      $startTime = ''
    }
    $rows += [pscustomobject]@{
      timestamp = (Get-Date).ToString('o')
      name = $process.ProcessName
      id = $process.Id
      responding = $process.Responding
      main_window_handle = $process.MainWindowHandle
      main_window_title = $process.MainWindowTitle
      start_time = $startTime
    }
  }

  if ($rows.Count -eq 0) {
    [pscustomobject]@{
      timestamp = (Get-Date).ToString('o')
      name = 'none'
      id = ''
      responding = ''
      main_window_handle = ''
      main_window_title = ''
      start_time = ''
    } | Export-Csv -LiteralPath $OutPath -NoTypeInformation -Encoding UTF8
    return
  }

  $rows | Export-Csv -LiteralPath $OutPath -NoTypeInformation -Encoding UTF8
}

function Capture-PrimaryScreen {
  param([string]$OutPath)

  Add-Type -AssemblyName System.Windows.Forms
  Add-Type -AssemblyName System.Drawing

  $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  try {
    $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
    $bitmap.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
  }
  finally {
    $graphics.Dispose()
    $bitmap.Dispose()
  }
}

$timestamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
$sessionDir = Join-Path $OutputRoot "phase7-vpn-$timestamp"
New-Item -ItemType Directory -Force -Path $sessionDir | Out-Null

$resolvedStateDir = Resolve-StateDir
$logPath = Join-Path $resolvedStateDir 'ecnuvpn.log'
$logOffset = 0
if (Test-Path -LiteralPath $logPath) {
  $logOffset = (Get-Item -LiteralPath $logPath).Length
}

$uiShellExe = Join-Path $PackageRoot 'exv-ui.exe'
$startedProcessId = ''
if (-not $NoLaunch) {
  if (-not (Test-Path -LiteralPath $uiShellExe)) {
    throw "exv-ui.exe not found: $uiShellExe"
  }
  $process = Start-Process -FilePath $uiShellExe -WorkingDirectory $PackageRoot -PassThru
  $startedProcessId = $process.Id
}

$metadata = [ordered]@{
  timestamp = (Get-Date).ToString('o')
  repo_root = $repoRoot
  package_root = $PackageRoot
  state_dir = $resolvedStateDir
  log_path = $logPath
  output_dir = $sessionDir
  launched = (-not $NoLaunch)
  started_process_id = $startedProcessId
  monitor_seconds = $MonitorSeconds
  sample_interval_ms = $SampleIntervalMs
  include_raw_log_delta = [bool]$IncludeRawLogDelta
  screenshot_requested = [bool]$CaptureScreenshot
}
$metadata | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $sessionDir 'metadata.json') -Encoding UTF8

Write-Host ''
Write-Host '=== ECNU-VPN Phase 7 Manual Verification Capture ===' -ForegroundColor Cyan
Write-Host "Output: $sessionDir"
Write-Host "State dir: $resolvedStateDir"
Write-Host "Log: $logPath"
Write-Host ''
Write-Host 'During the monitor window, perform the manual checks:'
Write-Host '  1. Click Connect and open Logs while connect is in progress.'
Write-Host '  2. Click the yellow in-progress cancel button.'
Write-Host '  3. Rapidly click connect/cancel several times.'
Write-Host '  4. Rapidly toggle minimal/advanced mode, including 8+ clicks.'
Write-Host '  5. If a real connect failure occurs, verify the visible UI error.'
Write-Host ''
if ($IncludeRawLogDelta) {
  Write-Host 'Raw log-delta capture is enabled; secret-like fields are redacted before writing.' -ForegroundColor Yellow
}
else {
  Write-Host 'Raw log-delta capture is disabled. Only a redacted connect-stage summary will be written.'
}
Write-Host "Monitoring process responsiveness for $MonitorSeconds seconds..."

$samplesPath = Join-Path $sessionDir 'process-samples.csv'
$samples = @()
$deadline = (Get-Date).AddSeconds($MonitorSeconds)
while ((Get-Date) -lt $deadline) {
  $now = Get-Date
  $processes = Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessName -in @('exv-ui', 'exv', 'exv-helper') }

  if ($processes.Count -eq 0) {
    $samples += [pscustomobject]@{
      timestamp = $now.ToString('o')
      name = 'none'
      id = ''
      responding = ''
      main_window_handle = ''
      main_window_title = ''
    }
  }
  else {
    foreach ($process in $processes) {
      $samples += [pscustomobject]@{
        timestamp = $now.ToString('o')
        name = $process.ProcessName
        id = $process.Id
        responding = $process.Responding
        main_window_handle = $process.MainWindowHandle
        main_window_title = $process.MainWindowTitle
      }
    }
  }

  Start-Sleep -Milliseconds $SampleIntervalMs
}

$samples | Export-Csv -LiteralPath $samplesPath -NoTypeInformation -Encoding UTF8
Write-ProcessSnapshot -OutPath (Join-Path $sessionDir 'process-final.csv')
Write-LogEvidence `
  -Path $logPath `
  -Offset $logOffset `
  -SummaryPath (Join-Path $sessionDir 'log-summary.txt') `
  -RawPath (Join-Path $sessionDir 'log-delta-redacted.txt') `
  -IncludeRaw ([bool]$IncludeRawLogDelta)

if ($CaptureScreenshot) {
  Capture-PrimaryScreen -OutPath (Join-Path $sessionDir 'primary-screen.png')
}

$observationTemplate = @'
# Phase 7 Manual Observation

- [ ] Connect attempt was started.
- [ ] Logs panel opened while connect was in progress.
- [ ] Logs/status/config UI stayed responsive during connect.
- [ ] Yellow in-progress button cancelled connect without showing a failure modal.
- [ ] Rapid connect/cancel settled to the latest user intent.
- [ ] Rapid minimal/advanced toggles did not bounce after input stopped.
- [ ] Final mode matched the last accepted user click.
- [ ] Real connect failure, if produced, was visible in the UI.
- [ ] Any unresponsive interval is visible in process-samples.csv.

Notes:

'@
$observationTemplate | Set-Content -LiteralPath (Join-Path $sessionDir 'manual-observation.md') -Encoding UTF8

Write-Host ''
Write-Host 'Capture complete.' -ForegroundColor Green
Write-Host "Artifacts: $sessionDir"
Write-Host 'Fill manual-observation.md with the observed result before using this as Phase 7 evidence.'
