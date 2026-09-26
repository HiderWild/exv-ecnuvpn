[CmdletBinding(SupportsShouldProcess = $true)]
param(
  [switch]$Force,
  [switch]$IncludeCredentialManager,
  [string]$LocalAppDataRoot = "",
  [string]$ProgramDataRoot = "",
  [string]$UserProfileRoot = "",
  [string]$RoamingAppDataRoot = "",
  [string]$ConfigDir = "",
  [string]$TempRoot = "",
  [string]$SystemTempRoot = "",
  [string]$InstallDir = "",
  [switch]$NoProcessStop
)

$ErrorActionPreference = 'Stop'
$cleanupErrors = New-Object 'System.Collections.Generic.List[string]'

function Add-CleanupError {
  param([string]$Path, [string]$Reason)
  $message = "$Path : $Reason"
  $script:cleanupErrors.Add($message)
  Write-Warning $message
}

function Zh {
  param([Parameter(Mandatory = $true)][string]$Base64)
  return [System.Text.Encoding]::UTF8.GetString(
    [System.Convert]::FromBase64String($Base64))
}

function Get-UserProfileRoot {
  if ($UserProfileRoot) { return $UserProfileRoot }
  if ($env:USERPROFILE) {
    return $env:USERPROFILE
  }
  if ($env:HOMEDRIVE -and $env:HOMEPATH) {
    return "$($env:HOMEDRIVE)$($env:HOMEPATH)"
  }
  return ""
}

function Get-LocalAppDataRoot {
  if ($LocalAppDataRoot) {
    return $LocalAppDataRoot
  }
  if ($UserProfileRoot) {
    return Join-Path $UserProfileRoot 'AppData\Local'
  }
  if ($env:LOCALAPPDATA) {
    return $env:LOCALAPPDATA
  }
  $userProfile = Get-UserProfileRoot
  if ($userProfile) {
    return Join-Path $userProfile 'AppData\Local'
  }
  throw (Zh '5pyq6K6+572uIExPQ0FMQVBQREFUQSDlkowgVVNFUlBST0ZJTEXvvIzml6Dms5XlrprkvY0gRVhWIOeUqOaIt+mFjee9ruebruW9leOAgg==')
}

function Get-ProgramDataRoot {
  if ($ProgramDataRoot) {
    return $ProgramDataRoot
  }
  if ($env:ProgramData) {
    return $env:ProgramData
  }
  return 'C:\ProgramData'
}

function Get-FullPath {
  param([Parameter(Mandatory = $true)][string]$Path)
  return [System.IO.Path]::GetFullPath($Path)
}

function Normalize-FullPath {
  param([Parameter(Mandatory = $true)][string]$Path)
  return (Get-FullPath $Path).TrimEnd('\', '/')
}

function Test-PathWithinRoot {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  $rootFull = Normalize-FullPath $Root
  $pathFull = Normalize-FullPath $Path
  if ($pathFull.Equals($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
    return $true
  }
  return $pathFull.StartsWith($rootFull + '\', [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-PathUnderRoot {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  if (-not (Test-PathWithinRoot -Path $Path -Root $Root)) {
    throw ((Zh '5ouS57ud5Yig6ZmkIEVYViDmnKzlnLDnlKjmiLfmlbDmja7moLnnm67lvZXkuYvlpJbnmoTot6/lvoTvvJo=') + (Get-FullPath $Path))
  }
  # 不沿数据根或中间目录的 junction/symlink 删除外部文件。目录内部的链接
  # 由 Remove-Item 删除链接本身；这里保护显式逐文件清理的祖先路径。
  $probe = Get-FullPath $Path
  while ($probe) {
    if (Test-Path -LiteralPath $probe) {
      $probeItem = Get-Item -LiteralPath $probe -Force
      if (($probeItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Cleanup target crosses a reparse point; preserved: $probe"
      }
    }
    $parent = Split-Path -Parent $probe
    if (-not $parent -or $parent -eq $probe) { break }
    $probe = $parent
  }
}

function Assert-ConfigDirSafeForCleanup {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [string[]]$SharedRoots = @()
  )

  if ($Path -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$))') {
    throw "Config directory is relative; original working directory is unknown: $Path"
  }
  $pathFull = Normalize-FullPath $Path
  $driveRoot = [System.IO.Path]::GetPathRoot($pathFull)
  if ([string]::IsNullOrWhiteSpace($driveRoot)) {
    throw "Redirected config dir has no drive root: $Path"
  }

  $normalizedDriveRoot = $driveRoot.TrimEnd('\', '/')
  if ($pathFull.Equals($normalizedDriveRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Redirected config dir cannot be a drive root: $Path"
  }

  foreach ($sharedRoot in $SharedRoots) {
    if ([string]::IsNullOrWhiteSpace($sharedRoot)) {
      continue
    }

    if ($pathFull.Equals((Normalize-FullPath $sharedRoot), [StringComparison]::OrdinalIgnoreCase)) {
      throw "Config directory cannot equal a broad shared root: $Path"
    }
  }

  Assert-PathUnderRoot -Path $pathFull -Root $pathFull
}

function New-CleanupEntry {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  return [PSCustomObject]@{
    Path = $Path
    Root = $Root
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
    throw "Expected an EXV file but found a directory; preserved: $Path"
  }

  Remove-ConfigTarget -Path $Path -Root $Root
}

function Remove-EmptyDirectoryIfExists {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root
  )

  Assert-PathUnderRoot -Path $Path -Root $Root
  if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
    return
  }

  $child = Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if ($null -ne $child) {
    return
  }

  if ($PSCmdlet.ShouldProcess($Path, 'Remove empty EXV config directory')) {
    Remove-Item -LiteralPath $Path -Force
    Write-Host ((Zh '5bey5Yig6Zmk77ya') + $Path)
  }
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

function Add-ConfigCleanupTargets {
  param(
    [Parameter(Mandatory = $true)][ref]$PathTargets,
    [Parameter(Mandatory = $true)][ref]$TreeTargets,
    [Parameter(Mandatory = $true)][ref]$EmptyDirTargets,
    [Parameter(Mandatory = $true)][string]$ConfigDir,
    [Parameter(Mandatory = $true)][string]$Root
  )

  $profileWebView2Dir = Join-Path $ConfigDir 'WebView2'
  $PathTargets.Value += @(
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'config.json') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'config.json.tmp') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir '.key') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'key.bin') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'close-preference.json') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'exv.log') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'exv.pid') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'tunnel.js') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'route-ready') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'logs\aggregated.jsonl') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'exv-core-ipc-v1.registry.json') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'exv-core-ipc-v1.lock') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'exv-core-ipc-v1.sock') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'connect-attempt.json') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'connect-attempt.mutex') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'active-tunnel.mutex') -Root $Root)
  )
  $TreeTargets.Value += @(
    (New-CleanupEntry -Path (Join-Path $profileWebView2Dir 'Default\Local Storage') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $profileWebView2Dir 'EBWebView\Default\Local Storage') -Root $Root),
    (New-CleanupEntry -Path $profileWebView2Dir -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'WebView2.migrating') -Root $Root)
  )
  # 历史 C++ 原子文件命名来自 connection_attempt.cpp / tunnel_resource_lease.cpp。
  # 自定义配置目录只按完整名称识别，不用宽泛的 *.tmp 或前缀整树删除。
  $lockDirs = @((Join-Path $ConfigDir 'connect-attempt.lock'), (Join-Path $ConfigDir 'active-tunnel.lock'))
  try {
    Assert-PathUnderRoot -Path $ConfigDir -Root $Root
    if (Test-Path -LiteralPath $ConfigDir -PathType Container) {
      foreach ($child in (Get-ChildItem -LiteralPath $ConfigDir -Force)) {
        if (-not $child.PSIsContainer -and $child.Name -match '^connect-attempt\.json\.tmp\.[0-9]+$') {
          $PathTargets.Value += (New-CleanupEntry -Path $child.FullName -Root $Root)
        } elseif ($child.PSIsContainer -and $child.Name -match '^connect-attempt\.lock\.tmp\.attempt-[0-9]+-[1-9][0-9]*-[1-9][0-9]*$') {
          $lockDirs += $child.FullName
        }
      }
    }
    foreach ($lockDir in $lockDirs) {
      Assert-PathUnderRoot -Path $lockDir -Root $Root
      $PathTargets.Value += (New-CleanupEntry -Path (Join-Path $lockDir 'owner.json') -Root $Root)
      if (Test-Path -LiteralPath $lockDir -PathType Container) {
        foreach ($child in (Get-ChildItem -LiteralPath $lockDir -Force -File)) {
          if ($child.Name -match '^owner\.json\.tmp\.[0-9]+$') {
            $PathTargets.Value += (New-CleanupEntry -Path $child.FullName -Root $Root)
          }
        }
      }
      $EmptyDirTargets.Value += (New-CleanupEntry -Path $lockDir -Root $Root)
    }
  } catch {
    Add-CleanupError -Path $ConfigDir -Reason $_.Exception.Message
  }
  $EmptyDirTargets.Value += (New-CleanupEntry -Path $ConfigDir -Root $Root)
}

function Remove-CleanupEntries {
  param(
    [Parameter(Mandatory = $true)]$Entries,
    [switch]$Tree,
    [switch]$EmptyDirectory
  )

  $seen = @{}
  foreach ($entry in $Entries) {
    if ($null -eq $entry) {
      continue
    }

    $pathKey = Normalize-FullPath $entry.Path
    $rootKey = Normalize-FullPath $entry.Root
    $dedupeKey = "$rootKey|$pathKey"
    if ($seen.ContainsKey($dedupeKey)) {
      continue
    }
    $seen[$dedupeKey] = $true

    try {
      if ($EmptyDirectory) {
        Remove-EmptyDirectoryIfExists -Path $entry.Path -Root $entry.Root
      } elseif ($Tree) {
        Remove-ConfigTree -Path $entry.Path -Root $entry.Root
      } else {
        Remove-ConfigPath -Path $entry.Path -Root $entry.Root
      }
    } catch {
      Add-CleanupError -Path $entry.Path -Reason $_.Exception.Message
    }
  }
}

function Get-ExvCredentialTargets {
  param([string[]]$Lines)
  $targets = foreach ($line in $Lines) {
    # 标签随系统语言变化；只接受稳定 target 字段中的 EXV 命名空间。
    if ($line -match '^\s*[^:\r\n]+:\s*[^:\r\n]+:target=(EXV/[^\r\n]+?)\s*$') {
      $Matches[1].Trim()
    } elseif ($line -match '^\s*(?:Target|目标)\s*[:：]\s*(EXV/[^\r\n]+?)\s*$') {
      $Matches[1].Trim()
    }
  }
  $targets | Sort-Object -Unique
}

function Remove-ExvCredentialManagerEntries {
  if (-not $IncludeCredentialManager) {
    return
  }

  $cmdkey = Get-Command cmdkey.exe -ErrorAction SilentlyContinue
  if (-not $cmdkey) {
    throw '未找到 cmdkey.exe，无法完成 Windows 凭据清理。'
  }

  $listed = & $cmdkey.Path /list 2>$null
  if ($LASTEXITCODE -ne 0) {
    throw "Windows 凭据枚举失败，cmdkey 退出码：$LASTEXITCODE"
  }
  foreach ($target in (Get-ExvCredentialTargets -Lines $listed)) {
    if ($PSCmdlet.ShouldProcess($target, (Zh '5Yig6ZmkIEVYViBXaW5kb3dzIOWHreaNrueuoeeQhuWZqOadoeebrg=='))) {
      try {
        & $cmdkey.Path "/delete:$target" | Out-Null
        if ($LASTEXITCODE -ne 0) {
          throw "Windows 凭据删除失败，cmdkey 退出码：$LASTEXITCODE"
        }
        Write-Host ((Zh '5bey5Yig6Zmk5Yet5o2u77ya') + $target)
      } catch {
        Add-CleanupError -Path "CredentialManager/$target" -Reason $_.Exception.Message
      }
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
  foreach ($name in @('LocalAppDataRoot', 'ProgramDataRoot', 'UserProfileRoot',
      'RoamingAppDataRoot', 'ConfigDir', 'InstallDir', 'TempRoot', 'SystemTempRoot')) {
    $value = Get-Variable -Name $name -ValueOnly
    if ($value) {
      $parts += "-$name"
      $parts += Quote-CommandArgument $value
    }
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

if (-not $NoProcessStop) {
  Stop-ExvUserProcesses
}

$localAppData = Get-LocalAppDataRoot
$userProfile = Get-UserProfileRoot
$roamingAppData = if ($RoamingAppDataRoot) { $RoamingAppDataRoot } elseif ($UserProfileRoot) {
  Join-Path $UserProfileRoot 'AppData\Roaming'
} else { $env:APPDATA }
$explicitConfigDir = if ($PSBoundParameters.ContainsKey('ConfigDir')) { $ConfigDir } elseif ($UserProfileRoot) {
  # 提权后环境可能属于另一管理员，不把该账户的 EXV_CONFIG_DIR 当成发起者配置。
  ''
} else { $env:EXV_CONFIG_DIR }
$programData = Get-ProgramDataRoot
$userTemp = if ($TempRoot) { $TempRoot } elseif ($UserProfileRoot) {
  Join-Path $localAppData 'Temp'
} else { [IO.Path]::GetTempPath() }
$systemTemp = if ($SystemTempRoot) { $SystemTempRoot } else {
  Join-Path ([Environment]::GetFolderPath('Windows')) 'Temp'
}
$appRoot = Join-Path $localAppData 'EXV'
$profileRoot = Join-Path $appRoot 'profile'
$profileDir = Join-Path $profileRoot 'default'
$redirectPath = Join-Path $appRoot 'profile.redirect'
$programsRoot = Join-Path $localAppData 'Programs\EXV'
$appWebView2Dir = Join-Path $programsRoot 'exv-ui.exe.WebView2'
$rustConfigDir = if ($userProfile) { Join-Path $userProfile '.exv' } else { '' }
$localResourceRoot = Join-Path $localAppData 'ExvVpn'
$machineResourceRoot = Join-Path $programData 'ExvVpn'
$serviceStateRoot = Join-Path $programData 'exv'
$legacyHelperRoot = Join-Path $programData 'EXV\Helper'

$pathTargets = @()
$treeTargets = @()
$emptyDirTargets = @()

Add-ConfigCleanupTargets -PathTargets ([ref]$pathTargets) `
  -TreeTargets ([ref]$treeTargets) `
  -EmptyDirTargets ([ref]$emptyDirTargets) `
  -ConfigDir $profileDir `
  -Root $appRoot

$pathTargets += (New-CleanupEntry -Path $redirectPath -Root $appRoot)
$pathTargets += (New-CleanupEntry -Path (Join-Path $programData 'exv-helper-session.json') -Root $programData)
$pathTargets += (New-CleanupEntry -Path (Join-Path $serviceStateRoot 'service.key') -Root $programData)
$machineLegacyProfile = Join-Path $serviceStateRoot 'profile\default'
$pathTargets += (New-CleanupEntry -Path (Join-Path $machineLegacyProfile 'exv.log') -Root $programData)
$treeTargets += @(
  # The exact roots below are owned by EXV.  This is intentionally root-level
  # cleanup after the user opted in, so unknown future state does not survive a
  # reinstall merely because it was not in a file-by-file allow list.
  (New-CleanupEntry -Path $appRoot -Root $localAppData),
  (New-CleanupEntry -Path $localResourceRoot -Root $localAppData),
  (New-CleanupEntry -Path $machineResourceRoot -Root $programData),
  (New-CleanupEntry -Path (Join-Path $userTemp 'ExvVpn') -Root $userTemp),
  (New-CleanupEntry -Path (Join-Path $systemTemp 'ExvVpn') -Root $systemTemp),
  (New-CleanupEntry -Path $legacyHelperRoot -Root (Join-Path $programData 'EXV')),
  (New-CleanupEntry -Path (Join-Path $appWebView2Dir 'EBWebView\Default\Local Storage') -Root $programsRoot),
  (New-CleanupEntry -Path $appWebView2Dir -Root $programsRoot)
)
if ($rustConfigDir) {
  $treeTargets += (New-CleanupEntry -Path $rustConfigDir -Root $userProfile)
}
if ($InstallDir) {
  $installRoot = Get-FullPath $InstallDir
  $treeTargets += (New-CleanupEntry -Path (Join-Path $installRoot 'exv-ui.exe.WebView2') -Root $installRoot)
}
$emptyDirTargets += (New-CleanupEntry -Path $profileRoot -Root $appRoot)
# Windows 的 exv/EXV 大小写不区分；服务密钥与旧 Helper 共用父目录。
# 只删确知叶和 Helper 子树，父目录仅在空时删除。
$emptyDirTargets += @(
  (New-CleanupEntry -Path $machineLegacyProfile -Root $programData),
  (New-CleanupEntry -Path (Join-Path $serviceStateRoot 'profile') -Root $programData),
  (New-CleanupEntry -Path $serviceStateRoot -Root $programData)
)

# EXV_CONFIG_DIR 是 Rust 配置层支持的显式位置；自定义目录可能含其他文件，
# 只清除已知 EXV 条目，不把整个目录当作 EXV 私有目录递归删除。
if ($explicitConfigDir) {
  try {
    Assert-ConfigDirSafeForCleanup -Path $explicitConfigDir -SharedRoots @(
      $userProfile, $localAppData, $roamingAppData, $programData, $env:SystemRoot, $env:ProgramFiles)
    $customConfigDir = Normalize-FullPath $explicitConfigDir
    Add-ConfigCleanupTargets -PathTargets ([ref]$pathTargets) `
      -TreeTargets ([ref]$treeTargets) -EmptyDirTargets ([ref]$emptyDirTargets) `
      -ConfigDir $customConfigDir -Root $customConfigDir
  } catch {
    Add-CleanupError -Path $explicitConfigDir -Reason $_.Exception.Message
  }
}

$redirectedConfigDir = ''
try {
  Assert-PathUnderRoot -Path $redirectPath -Root $appRoot
  $redirectedConfigDir = Get-RedirectedConfigDir -RedirectPath $redirectPath -UserProfile $userProfile
} catch {
  Add-CleanupError -Path $redirectPath -Reason $_.Exception.Message
}
if ($redirectedConfigDir) {
  try {
    Assert-ConfigDirSafeForCleanup -Path $redirectedConfigDir -SharedRoots @(
      $appRoot,
      $localAppData,
      $roamingAppData,
      $userProfile
      $programData
      $env:SystemRoot
      $env:ProgramFiles
    )
    $redirectedConfigDir = Get-FullPath $redirectedConfigDir
    Add-ConfigCleanupTargets -PathTargets ([ref]$pathTargets) `
      -TreeTargets ([ref]$treeTargets) `
      -EmptyDirTargets ([ref]$emptyDirTargets) `
      -ConfigDir $redirectedConfigDir `
      -Root $redirectedConfigDir
  } catch {
    Add-CleanupError -Path $redirectedConfigDir -Reason $_.Exception.Message
  }
}

Remove-CleanupEntries -Entries $pathTargets
Remove-CleanupEntries -Entries $treeTargets -Tree
Remove-CleanupEntries -Entries $emptyDirTargets -EmptyDirectory

try {
  Remove-ExvCredentialManagerEntries
} catch {
  Add-CleanupError -Path 'Windows Credential Manager' -Reason $_.Exception.Message
}
if ($cleanupErrors.Count -gt 0) {
  throw ("EXV cleanup incomplete ({0} errors):`n{1}" -f $cleanupErrors.Count, ($cleanupErrors -join "`n"))
}
