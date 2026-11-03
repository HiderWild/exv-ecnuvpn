[CmdletBinding(SupportsShouldProcess = $true)]
param(
  [switch]$Force,
  [switch]$IncludeCredentialManager,
  # KI-13 ④：升级语义——照常清理缓存/账本/服务态，但跳过用户数据段
  # （连接配置、凭据、UI 偏好、历代安装根内可能存在的 WebView2 用户数据）。
  # 全新卸载（卸载链默认）不传该开关：全清。
  [switch]$PreserveUserData,
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
# KI-13 ③：被锁文件成功登记 MoveFileEx 重启删除的数量。脚本以摘要行
# 「EXV-CLEANUP-SUMMARY: scheduled=<n>」向调用方（卸载器）报告；全清或部分
# 待重启都是成功语义，硬失败（重试与登记都做不到）才计入 $cleanupErrors 并
# 以非零退出码结束。
$script:scheduledRebootDeletes = 0

# KI-13 ③：有限重试参数——3 次、间隔 500ms。重试仍锁才登记重启删除。
$script:LockRetryCount = 3
$script:LockRetryDelayMs = 500

# MOVEFILE_DELAY_UNTIL_REBOOT：登记到 HKLM PendingFileRenameOperations，重启时
# 由会话管理器按登记顺序删除（本脚本先登记文件、后登记目录壳，顺序即删除顺序）。
# 需要管理员权限；卸载链以提权身份调用本脚本，手动运行未提权时登记失败按硬失败处理。
Add-Type -Namespace ExvCleanup -Name NativeMethods -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern bool MoveFileEx(string existingFileName, string newFileName, int flags);
'@
$script:MoveFileDelayUntilReboot = 4

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

# --- KI-13 ③：被锁文件的有限重试 + 重启删除登记 -------------------------------
# 语义分层（用户裁定 1/3）：
#   立即删除成功                       → 成功；
#   重试后成功                         → 成功；
#   仍锁但 MoveFileEx 重启删除登记成功 → 成功（摘要行报告 scheduled 计数）；
#   登记也失败                         → 硬失败（cleanupErrors → 非零退出）。

function Invoke-RebootDeleteRegistration {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Reason
  )

  if ([ExvCleanup.NativeMethods]::MoveFileEx($Path, $null, $script:MoveFileDelayUntilReboot)) {
    $script:scheduledRebootDeletes++
    Write-Host ("已登记重启删除（部分文件将在重启后自动删除）：" + $Path)
    return $true
  }
  $win32 = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
  Add-CleanupError -Path $Path -Reason ("reboot delete registration failed (Win32=$win32); reason: $Reason")
  return $false
}

function Remove-FileWithLockHandling {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Reason
  )

  for ($attempt = 1; $attempt -le $script:LockRetryCount; $attempt++) {
    try {
      Remove-Item -LiteralPath $Path -Force -ErrorAction Stop
      return $true
    } catch {
      if (-not (Test-Path -LiteralPath $Path)) {
        return $true
      }
      if ($attempt -lt $script:LockRetryCount) {
        Start-Sleep -Milliseconds $script:LockRetryDelayMs
      }
    }
  }
  if (-not (Test-Path -LiteralPath $Path)) {
    return $true
  }
  return (Invoke-RebootDeleteRegistration -Path $Path -Reason $Reason)
}

function Remove-TreeWithLockHandling {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Reason
  )

  # 第一阶段：整树直接删除 + 有限重试。多数被锁场景在进程退出后的重试窗口内解除。
  for ($attempt = 1; $attempt -le $script:LockRetryCount; $attempt++) {
    try {
      Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
      return $true
    } catch {
      if (-not (Test-Path -LiteralPath $Path)) {
        return $true
      }
      if ($attempt -lt $script:LockRetryCount) {
        Start-Sleep -Milliseconds $script:LockRetryDelayMs
      }
    }
  }
  if (-not (Test-Path -LiteralPath $Path)) {
    return $true
  }

  # 第二阶段：残留清点——先逐文件（删除或登记），再按深序处理目录壳。登记顺序
  # 即会话管理器重启时的删除顺序（文件在前、父目录在后），保证目录清空后可删。
  try {
    $remaining = @(Get-ChildItem -LiteralPath $Path -Recurse -Force -ErrorAction Stop)
  } catch {
    Add-CleanupError -Path $Path -Reason ("residual enumeration failed after retries; reason: $Reason ($($_.Exception.Message))")
    return $false
  }
  $remainingFiles = @($remaining | Where-Object { -not $_.PSIsContainer })
  $remainingDirs = @(
    $remaining |
      Where-Object { $_.PSIsContainer } |
      Sort-Object @{ Expression = { $_.FullName.Length } } -Descending
  )
  foreach ($file in $remainingFiles) {
    $null = (Remove-FileWithLockHandling -Path $file.FullName -Reason $Reason)
  }
  foreach ($dir in $remainingDirs) {
    if (Test-Path -LiteralPath $dir.FullName) {
      if (-not (Remove-FileWithLockHandling -Path $dir.FullName -Reason "$Reason (directory shell)")) {
        # 目录壳登记失败已由 Remove-FileWithLockHandling 记为硬失败；继续处理兄弟目录。
      }
    }
  }
  if (Test-Path -LiteralPath $Path) {
    return (Remove-FileWithLockHandling -Path $Path -Reason "$Reason (tree root)")
  }
  return $true
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
    if (Remove-FileWithLockHandling -Path $Path -Reason 'single-file cleanup') {
      Write-Host ((Zh '5bey5Yig6Zmk77ya') + $Path)
    }
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
    if (Remove-TreeWithLockHandling -Path $Path -Reason 'tree cleanup') {
      Write-Host ((Zh '5bey5Yig6Zmk77ya') + $Path)
    }
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
    if (Remove-FileWithLockHandling -Path $Path -Reason 'empty directory cleanup') {
      Write-Host ((Zh '5bey5Yig6Zmk77ya') + $Path)
    }
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
    # KI-13/A（2026-09-28 复审 F2）：快速入门「已见」标记与 config.json 同目录；
    # 默认 ~/.exv 走整树删除天然覆盖，重定向目录走本清单——必须显式列出。
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'quick-start.json') -Root $Root),
    (New-CleanupEntry -Path (Join-Path $ConfigDir 'quick-start.json.tmp') -Root $Root),
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
    # C++ 3.3.x 时代的 core 进程名（exv.exe）——历代进程名漂移兼容。
    Stop-Process -Name exv -Force -ErrorAction SilentlyContinue
  }
  if ($PSCmdlet.ShouldProcess('exv-core', (Zh '5YGc5q2iIEVYViBjb3JlIOi/m+eoiw=='))) {
    Stop-Process -Name exv-core -Force -ErrorAction SilentlyContinue
  }
  if ($PSCmdlet.ShouldProcess('exv-engine', '结束 EXV engine 进程')) {
    Stop-Process -Name exv-engine -Force -ErrorAction SilentlyContinue
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
    '-ExecutionPolicy',
    'Bypass',
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
  if ($PreserveUserData) {
    $parts += '-PreserveUserData'
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

# KI-13 ⑤：清单治理——EXV 专属根按「代码所有权」整树删除，不再按猜文件名逐叶列举。
# %LOCALAPPDATA%\EXV 是产品固定命名空间（profile\default、ui-preferences.json、
# profile.redirect、旧 Helper、历代 WebView2 状态都在其中），整树删除天然覆盖所有
# 历代漂移叶；逐叶清单只保留给「自定义/重定向配置目录」（那里可能有第三方文件，
# 只能删已知 EXV 叶）。未找到提示的日志噪音与漏删漂移叶的问题一并消除。

# KI-13 ④：PreserveUserData（升级语义）跳过用户数据段——连接配置（~/.exv）、
# UI 偏好与 profile（%LOCALAPPDATA%\EXV）、凭据管理器、自定义/重定向配置目录、
# 历代安装根（可能含 WebView2 用户数据）。缓存/账本/服务态照常清理。
$redirectedConfigDir = ''
if (-not $PreserveUserData) {
  # %LOCALAPPDATA%\EXV 整树（含 profile、ui-preferences.json、profile.redirect、旧 Helper）。
  $treeTargets += (
    # The exact roots below are owned by EXV.  This is intentionally root-level
    # cleanup after the user opted in, so unknown future state does not survive a
    # reinstall merely because it was not in a file-by-file allow list.
    (New-CleanupEntry -Path $appRoot -Root $localAppData)
  )
  if ($rustConfigDir) {
    # ~/.exv：连接配置、加密凭据、wintun 预置位、聚合日志（当前 Rust 配置根）。
    $treeTargets += (New-CleanupEntry -Path $rustConfigDir -Root $userProfile)
  }
} else {
  Write-Host "PreserveUserData: skip user-data trees ($appRoot, $rustConfigDir, credentials)"
}

$pathTargets += (New-CleanupEntry -Path (Join-Path $programData 'exv-helper-session.json') -Root $programData)
$pathTargets += (New-CleanupEntry -Path (Join-Path $serviceStateRoot 'service.key') -Root $programData)
$machineLegacyProfile = Join-Path $serviceStateRoot 'profile\default'
$pathTargets += (New-CleanupEntry -Path (Join-Path $machineLegacyProfile 'exv.log') -Root $programData)
$treeTargets += @(
  (New-CleanupEntry -Path $localResourceRoot -Root $localAppData),
  (New-CleanupEntry -Path $machineResourceRoot -Root $programData),
  (New-CleanupEntry -Path (Join-Path $userTemp 'ExvVpn') -Root $userTemp),
  (New-CleanupEntry -Path (Join-Path $systemTemp 'ExvVpn') -Root $systemTemp),
  (New-CleanupEntry -Path $legacyHelperRoot -Root (Join-Path $programData 'EXV')),
  # 安装根下的 WebView2 用户数据目录是缓存性状态：全新卸载清除；升级保留语义下
  # 也照清（升级覆盖安装本就会重建该目录）。
  (New-CleanupEntry -Path (Join-Path $appWebView2Dir 'EBWebView\Default\Local Storage') -Root $programsRoot),
  (New-CleanupEntry -Path $appWebView2Dir -Root $programsRoot)
)
if ($InstallDir) {
  $installRoot = Get-FullPath $InstallDir
  $treeTargets += (New-CleanupEntry -Path (Join-Path $installRoot 'exv-ui.exe.WebView2') -Root $installRoot)
}
$emptyDirTargets += @(
  (New-CleanupEntry -Path $machineLegacyProfile -Root $programData),
  (New-CleanupEntry -Path (Join-Path $serviceStateRoot 'profile') -Root $programData),
  (New-CleanupEntry -Path $serviceStateRoot -Root $programData)
)

# KI-13 ⑥ 历代安装根漂移治理：ECNU VPN/ECNU-VPN 时代的 per-user 安装位与
# C++ 时代的 Program Files 安装位（见 scripts/clear-windows-legacy-state.ps1 的
# 考古）。仅在卸载链上下文（显式 -InstallDir）且非保留语义时整树清理；当前
# 安装目录（及其后代）交给卸载器引擎处理，脚本侧绝不触碰——否则会连同活动
# 卸载器映像一起登记重启删除，破坏同路径重装安全性。
if ($InstallDir -and -not $PreserveUserData) {
  $currentInstallRoot = Normalize-FullPath $InstallDir
  $legacyInstallParents = @()
  if ($localAppData) {
    $legacyInstallParents += ,@((Join-Path $localAppData 'Programs'), $localAppData)
  }
  if ($env:ProgramFiles) {
    $legacyInstallParents += ,@($env:ProgramFiles, $env:ProgramFiles)
  }
  if (${env:ProgramFiles(x86)}) {
    $legacyInstallParents += ,@(${env:ProgramFiles(x86)}, ${env:ProgramFiles(x86)})
  }
  foreach ($pair in $legacyInstallParents) {
    $base = $pair[0]
    $root = $pair[1]
    foreach ($name in @('EXV', 'ECNU VPN', 'ECNU-VPN')) {
      $candidate = Join-Path $base $name
      $candidateNorm = Normalize-FullPath $candidate
      if ($candidateNorm.Equals($currentInstallRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        continue
      }
      if ($currentInstallRoot.StartsWith($candidateNorm + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
        continue
      }
      $treeTargets += (New-CleanupEntry -Path $candidate -Root $root)
    }
  }
}

# EXV_CONFIG_DIR 是 Rust 配置层支持的显式位置；自定义目录可能含其他文件，
# 只清除已知 EXV 条目，不把整个目录当作 EXV 私有目录递归删除。
if ($explicitConfigDir -and -not $PreserveUserData) {
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

# profile.redirect 的重定向目标（C++ 时代配置根）：必须在 %LOCALAPPDATA%\EXV
# 整树删除之前解析。保留语义下跳过（重定向目标属于用户数据）。
if (-not $PreserveUserData) {
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
}

Remove-CleanupEntries -Entries $pathTargets
Remove-CleanupEntries -Entries $treeTargets -Tree
Remove-CleanupEntries -Entries $emptyDirTargets -EmptyDirectory

if (-not $PreserveUserData) {
  try {
    Remove-ExvCredentialManagerEntries
  } catch {
    Add-CleanupError -Path 'Windows Credential Manager' -Reason $_.Exception.Message
  }
}

if ($cleanupErrors.Count -gt 0) {
  # KI-13 ③：只有硬失败（重试与重启删除登记都做不到）才非零退出。
  throw ("EXV cleanup incomplete ({0} errors):`n{1}" -f $cleanupErrors.Count, ($cleanupErrors -join "`n"))
}

# 成功摘要行（全清或部分待重启）：供卸载器汇总「部分文件将在重启后自动清理」
# 的提示性文案；[Console]::WriteLine 保证进入真实 stdout（不随 Write-Host 流配置变化）。
[Console]::WriteLine("EXV-CLEANUP-SUMMARY: scheduled=$scheduledRebootDeletes")
