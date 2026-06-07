param(
  [switch]$SkipDesktop,
  [switch]$DesktopSmoke
)

$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir

function Invoke-Step {
  param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Arguments
  )

  & $FilePath @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "Command failed: $FilePath $($Arguments -join ' ')"
  }
}

Push-Location $repoRoot
try {
  Write-Host '[merge-prep] Build frontend assets for native embedding...'
  Push-Location (Join-Path $repoRoot 'webui')
  try {
    Invoke-Step pnpm run build
  }
  finally {
    Pop-Location
  }

  Write-Host '[merge-prep] Configure and build native targets...'
  Invoke-Step cmake --preset windows-release
  Invoke-Step cmake --build --preset windows-release --target exv exv-helper platform_status_models_test backend_resolver_test vpn_runtime_test tunnel_script_contract_test proxy_tun_detector_test app_api_runtime_policy_test crypto_roundtrip_test

  Write-Host '[merge-prep] Run focused native regression tests...'
  Invoke-Step ctest --preset windows-release -R 'platform_status_models_test|backend_resolver_test|vpn_runtime_test|tunnel_script_contract_test|proxy_tun_detector_test|app_api_runtime_policy_test|crypto_roundtrip_test' --output-on-failure
}
finally {
  Pop-Location
}

if (-not $SkipDesktop) {
  Write-Host '[merge-prep] Compile Electron main/preload and native staging...'
  Push-Location (Join-Path $repoRoot 'webui')
  try {
    Invoke-Step pnpm run build:electron
    Invoke-Step pnpm run prepare:native
  }
  finally {
    Pop-Location
  }
}

if ($DesktopSmoke) {
  Write-Host '[merge-prep] Run desktop debug smoke build and launch...'
  Invoke-Step powershell -ExecutionPolicy Bypass -File (Join-Path $scriptDir 'build-windows.ps1') -Action debug-run
}

Write-Host '[merge-prep] Windows validation complete.'
