param(
  [ValidateSet('cpp', 'test', 'webview', 'electron', 'debug', 'debug-run', 'desktop', 'all', 'clean')]
  [string]$Action = 'all'
)

$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir
$buildRoot = Join-Path $repoRoot 'build\windows'
$cppBuildRoot = Join-Path $repoRoot 'build-windows'
$env:ECNUVPN_BUILD_PLATFORM = 'windows'
$env:ECNUVPN_WEBUI_DIST_DIR = Join-Path $buildRoot 'webview\dist'

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

function Invoke-CppBuild {
  param(
    [switch]$UiShell
  )

  Push-Location $repoRoot
  try {
    if ($UiShell) {
      Invoke-Step cmake --preset windows-release -DEXV_BUILD_UI_SHELL=ON
    }
    else {
      Invoke-Step cmake --preset windows-release
    }
    Invoke-Step cmake --build --preset windows-release --target exv exv-helper exv-ui platform_status_models_test backend_resolver_test vpn_runtime_test native_packaging_policy_test ui_shell_contract_test ui_shell_core_rpc_client_test ui_shell_cmake_policy_test win32_webview2_runtime_test
  }
  finally {
    Pop-Location
  }
}

function Invoke-CppTests {
  Push-Location $repoRoot
  try {
    Invoke-Step ctest --preset windows-release -R 'platform_status_models_test|backend_resolver_test|vpn_runtime_test|native_packaging_policy_test|ui_shell_contract_test|ui_shell_core_rpc_client_test|ui_shell_cmake_policy_test|win32_webview2_runtime_test'
  }
  finally {
    Pop-Location
  }
}

function Invoke-WebuiRendererBuild {
  Push-Location (Join-Path $repoRoot 'webui')
  try {
    Invoke-Step pnpm run webview:compile
  }
  finally {
    Pop-Location
  }
}

function Invoke-ElectronCompile {
  Push-Location (Join-Path $repoRoot 'webui')
  try {
    Invoke-Step pnpm run desktop:compile
  }
  finally {
    Pop-Location
  }
}

function Invoke-WebViewPackage {
  Push-Location (Join-Path $repoRoot 'webui')
  try {
    Invoke-Step pnpm run webview:package
  }
  finally {
    Pop-Location
  }
}

function Invoke-DesktopDebugBuild {
  Push-Location (Join-Path $repoRoot 'webui')
  try {
    Invoke-Step pnpm run desktop:package:dir
  }
  finally {
    Pop-Location
  }
}

function Clear-DesktopRelease {
  $releaseRoot = Join-Path $buildRoot 'electron\release'
  if (Test-Path $releaseRoot) {
    Remove-Item -Recurse -Force $releaseRoot
  }
}

function Invoke-DesktopDebugLaunch {
  $releaseRoot = Join-Path $buildRoot 'electron\release'
  $candidate = Join-Path $releaseRoot 'win-unpacked\ECNU-VPN.exe'

  if (-not (Test-Path $candidate)) {
    $candidate = Get-ChildItem -Path $releaseRoot -Filter '*.exe' -Recurse -File |
      Where-Object { $_.FullName -match 'win-unpacked' } |
      Select-Object -First 1 -ExpandProperty FullName
  }

  if (-not $candidate) {
    throw "Unpacked debug executable not found under $releaseRoot"
  }

  Start-Process -FilePath $candidate
}

switch ($Action) {
  'cpp' {
    Invoke-CppBuild
  }
  'test' {
    Invoke-CppTests
  }
  'electron' {
    Invoke-ElectronCompile
  }
  'webview' {
    Invoke-WebuiRendererBuild
    Invoke-CppBuild -UiShell
    Invoke-CppTests
    Invoke-WebViewPackage
  }
  'debug' {
    Invoke-CppBuild
    Invoke-CppTests
    Invoke-ElectronCompile
    Clear-DesktopRelease
    Invoke-DesktopDebugBuild
  }
  'debug-run' {
    Invoke-CppBuild
    Invoke-CppTests
    Invoke-ElectronCompile
    Clear-DesktopRelease
    Invoke-DesktopDebugBuild
    Invoke-DesktopDebugLaunch
  }
  'desktop' {
    Invoke-WebuiRendererBuild
    Invoke-CppBuild -UiShell
    Invoke-CppTests
    Invoke-WebViewPackage
  }
  'all' {
    Invoke-WebuiRendererBuild
    Invoke-CppBuild -UiShell
    Invoke-CppTests
    Invoke-WebViewPackage
  }
  'clean' {
    if (Test-Path $buildRoot) {
      Remove-Item -Recurse -Force $buildRoot
    }
    if (Test-Path $cppBuildRoot) {
      Remove-Item -Recurse -Force $cppBuildRoot
    }
  }
}
