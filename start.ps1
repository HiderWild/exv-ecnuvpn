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

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$scriptDir = Join-Path $repoRoot 'scripts'
. (Join-Path $scriptDir 'start-common.ps1') -RepoRoot $repoRoot -Port $Port -WebView2SdkDir $WebView2SdkDir

try {
  Set-ProtectedProcessIds (Get-CurrentProcessAncestorIds)

  if ($Status) {
    Show-Status
    exit 0
  }

  Invoke-PrivilegedMaintenanceScript -PauseOnError:$PauseOnError

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
    Start-WebViewShell -PauseOnError:$PauseOnError
  }
  else {
    Write-Info 'Build completed without launching native WebView shell'
  }
}
catch {
  Write-Error $_
  if ($PauseOnError) {
    Read-Host 'Press Enter to close this window'
  }
  exit 1
}
