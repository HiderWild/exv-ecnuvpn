[CmdletBinding(SupportsShouldProcess = $true)]
param(
  [switch]$Force,
  [switch]$IncludeCredentialManager,
  [string]$LocalAppDataRoot = ""
)

$ErrorActionPreference = 'Stop'

function Zh {
  param([Parameter(Mandatory = $true)][string]$Base64)
  return [System.Text.Encoding]::UTF8.GetString(
    [System.Convert]::FromBase64String($Base64))
}

function Get-LocalAppDataRoot {
  if ($LocalAppDataRoot) {
    return $LocalAppDataRoot
  }
  if ($env:LOCALAPPDATA) {
    return $env:LOCALAPPDATA
  }
  if ($env:USERPROFILE) {
    return Join-Path $env:USERPROFILE 'AppData\Local'
  }
  throw (Zh '5pyq6K6+572uIExPQ0FMQVBQREFUQSDlkowgVVNFUlBST0ZJTEXvvIzml6Dms5XlrprkvY0gRVhWIOeUqOaIt+mFjee9ruebruW9leOAgg==')
}

function Get-FullPath {
  param([Parameter(Mandatory = $true)][string]$Path)
  return [System.IO.Path]::GetFullPath($Path)
}

function Assert-PathUnderRoot {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  $rootFull = Get-FullPath $Root
  $pathFull = Get-FullPath $Path
  $rootFull = $rootFull.TrimEnd('\', '/')

  if ($pathFull.Equals($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    return
  }
  if (-not $pathFull.StartsWith($rootFull + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
    throw ((Zh '5ouS57ud5Yig6ZmkIEVYViDmnKzlnLDnlKjmiLfmlbDmja7moLnnm67lvZXkuYvlpJbnmoTot6/lvoTvvJo=') + $pathFull)
  }
}

function Remove-ConfigTarget {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  Assert-PathUnderRoot -Path $Path -Root $Root
  if (-not (Test-Path -LiteralPath $Path)) {
    Write-Host ((Zh '5pyq5om+5Yiw77ya') + $Path)
    return
  }

  if ($PSCmdlet.ShouldProcess($Path, (Zh '5Yig6ZmkIEVYViDmnKzlnLDnlKjmiLfphY3nva7mlofku7Y='))) {
    Remove-Item -LiteralPath $Path -Force
    Write-Host ((Zh '5bey5Yig6Zmk77ya') + $Path)
  }
}

function Remove-ConfigTree {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  Assert-PathUnderRoot -Path $Path -Root $Root
  if (-not (Test-Path -LiteralPath $Path)) {
    Write-Host ((Zh '5pyq5om+5Yiw77ya') + $Path)
    return
  }

  if ($PSCmdlet.ShouldProcess($Path, (Zh '5Yig6ZmkIEVYViDmnKzlnLDnlKjmiLfphY3nva7nm67lvZU='))) {
    Remove-Item -LiteralPath $Path -Recurse -Force
    Write-Host ((Zh '5bey5Yig6Zmk77ya') + $Path)
  }
}

function Remove-ConfigPath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  Assert-PathUnderRoot -Path $Path -Root $Root
  if (-not (Test-Path -LiteralPath $Path)) {
    Write-Host ((Zh '5pyq5om+5Yiw77ya') + $Path)
    return
  }

  $item = Get-Item -LiteralPath $Path -Force
  if ($item.PSIsContainer) {
    Remove-ConfigTree -Path $Path -Root $Root
    return
  }

  Remove-ConfigTarget -Path $Path -Root $Root
}

function Get-RedirectedConfigDir {
  param(
    [Parameter(Mandatory = $true)][string]$RedirectPath,
    [Parameter(Mandatory = $true)][string]$UserProfile
  )

  if (-not (Test-Path -LiteralPath $RedirectPath)) {
    return ""
  }

  $content = (Get-Content -LiteralPath $RedirectPath -ErrorAction Stop | Select-Object -First 1)
  $dir = ''
  if ($null -ne $content) {
    $dir = $content.Trim()
  }
  if (-not $dir) {
    return ""
  }
  if ($dir.StartsWith('~') -and $UserProfile) {
    return Join-Path $UserProfile $dir.Substring(1)
  }
  return $dir
}

function Remove-ExvCredentialManagerEntries {
  if (-not $IncludeCredentialManager) {
    return
  }

  $cmdkey = Get-Command cmdkey.exe -ErrorAction SilentlyContinue
  if (-not $cmdkey) {
    Write-Warning (Zh '5pyq5om+5YiwIGNtZGtleS5leGXvvJvlt7Lot7Pov4cgV2luZG93cyDlh63mja7nrqHnkIblmajmuIXnkIbjgII=')
    return
  }

  $listed = & $cmdkey.Path /list 2>$null
  $targets = @()
  foreach ($line in $listed) {
    if ($line -match '^\s*Target:\s*(.+)\s*$') {
      $target = $Matches[1].Trim()
      if ($target -match 'target=(.+)$') {
        $target = $Matches[1].Trim()
      }
      if ($target -like 'EXV/*') {
        $targets += $target
      }
    }
  }

  foreach ($target in ($targets | Sort-Object -Unique)) {
    if ($PSCmdlet.ShouldProcess($target, (Zh '5Yig6ZmkIEVYViBXaW5kb3dzIOWHreaNrueuoeeQhuWZqOadoeebrg=='))) {
      & $cmdkey.Path "/delete:$target" | Out-Null
      Write-Host ((Zh '5bey5Yig6Zmk5Yet5o2u77ya') + $target)
    }
  }
}

function Stop-ExvUserProcesses {
  if ($PSCmdlet.ShouldProcess('exv-ui', (Zh '5YGc5q2iIEVYViBVSSDov5vnqIs='))) {
    Stop-Process -Name exv-ui -Force -ErrorAction SilentlyContinue
  }
  if ($PSCmdlet.ShouldProcess('exv', (Zh '5YGc5q2iIEVYViBjb3JlIOi/m+eoiw=='))) {
    Stop-Process -Name exv -Force -ErrorAction SilentlyContinue
  }
}

function Quote-CommandArgument {
  param([Parameter(Mandatory = $true)][string]$Value)
  return '"' + $Value.Replace('"', '`"') + '"'
}

function Get-ForceCommandText {
  $parts = @(
    'powershell.exe',
    '-NoProfile',
    '-ExecutionPolicy Bypass',
    '-File',
    (Quote-CommandArgument $PSCommandPath),
    '-Force'
  )
  if ($LocalAppDataRoot) {
    $parts += '-LocalAppDataRoot'
    $parts += Quote-CommandArgument $LocalAppDataRoot
  }
  if ($IncludeCredentialManager) {
    $parts += '-IncludeCredentialManager'
  }
  return ($parts -join ' ')
}

if (-not $Force -and -not $WhatIfPreference) {
  $WhatIfPreference = $true
  Write-Host (Zh '5b2T5YmN5LuF6aKE5ryU77yM5LiN5Lya5Yig6Zmk5paH5Lu244CC56Gu6K6k5YiX6KGo5peg6K+v5ZCO77yM5L2/55SoIC1Gb3JjZSDmiafooYzmuIXnkIbjgII=')
  Write-Host ((Zh '5Y+v5aSN5Yi25ZG95Luk77ya') + (Get-ForceCommandText))
}

Stop-ExvUserProcesses

$localAppData = Get-LocalAppDataRoot
$appRoot = Join-Path $localAppData 'EXV'
$profileDir = Join-Path $appRoot 'profile\default'
$redirectPath = Join-Path $appRoot 'profile.redirect'
$programsRoot = Join-Path $localAppData 'Programs\EXV'
$appWebView2Dir = Join-Path $programsRoot 'exv-ui.exe.WebView2'
$profileWebView2Dir = Join-Path $profileDir 'WebView2'
$targets = @(
  (Join-Path $profileDir 'config.json'),
  (Join-Path $profileDir '.key'),
  (Join-Path $profileDir 'close-preference.json'),
  (Join-Path $profileDir 'exv.log'),
  (Join-Path $profileDir 'exv-core-ipc-v1.registry.json'),
  (Join-Path $profileDir 'exv-core-ipc-v1.lock'),
  (Join-Path $profileDir 'exv-core-ipc-v1.sock'),
  (Join-Path $profileDir 'connect-attempt.json'),
  (Join-Path $profileDir 'connect-attempt.mutex'),
  $redirectPath
)
$treeTargets = @(
  (Join-Path $profileDir 'connect-attempt.lock'),
  (Join-Path $appWebView2Dir 'EBWebView\Default\Local Storage'),
  $appWebView2Dir,
  (Join-Path $profileWebView2Dir 'Default\Local Storage'),
  (Join-Path $profileWebView2Dir 'EBWebView\Default\Local Storage'),
  $profileWebView2Dir
)

$redirectedConfigDir = Get-RedirectedConfigDir -RedirectPath $redirectPath -UserProfile $env:USERPROFILE
if ($redirectedConfigDir) {
  try {
    Assert-PathUnderRoot -Path $redirectedConfigDir -Root $appRoot
    $targets += Join-Path $redirectedConfigDir 'config.json'
    $targets += Join-Path $redirectedConfigDir '.key'
    $targets += Join-Path $redirectedConfigDir 'close-preference.json'
    $targets += Join-Path $redirectedConfigDir 'exv.log'
  } catch {
    Write-Warning ((Zh '5bey6Lez6L+HIA==') + $appRoot + (Zh 'IOS5i+WklueahOmHjeWumuWQkemFjee9ruebruW9le+8mg==') + $redirectedConfigDir)
  }
}

foreach ($target in ($targets | Select-Object -Unique)) {
  Remove-ConfigPath -Path $target -Root $appRoot
}

foreach ($target in ($treeTargets | Select-Object -Unique)) {
  $root = if ($target.StartsWith($programsRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    $programsRoot
  } else {
    $appRoot
  }
  Remove-ConfigTree -Path $target -Root $root
}

Remove-ExvCredentialManagerEntries
