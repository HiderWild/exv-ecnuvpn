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
  [string]$BrowserDataDir = "",
  [string]$TempRoot = "",
  [string]$SystemTempRoot = "",
  [string]$InstallDir = "",
  [switch]$NoProcessStop,
  # S3c（S3a -ManifestPath 约定）：安装文件清单库路径。未传时按两候选探测（先随包
  # 后仓内），两候选都不存在视为清单缺失，fail-closed 硬失败——见 Get-InstallManifestPath。
  [string]$ManifestPath = ""
)

$ErrorActionPreference = 'Stop'
$cleanupErrors = New-Object 'System.Collections.Generic.List[string]'
# KI-13 ③：只有移至唯一隔离路径的锁文件才允许登记 MoveFileEx 重启删除。
# 成功登记数量以摘要行
# 「EXV-CLEANUP-SUMMARY: scheduled=<n>」向调用方（卸载器）报告；全清或部分
# 待重启都是成功语义，硬失败（重试与登记都做不到）才计入 $cleanupErrors 并
# 以非零退出码结束。
$script:scheduledRebootDeletes = 0
$script:isolatedCleanupRoots = New-Object 'System.Collections.Generic.List[string]'

# KI-13 ③：有限重试参数——3 次、间隔 500ms。重试仍锁先隔离；
# 隔离失败报硬失败，不登记可能被新安装复用的原路径。
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
    [Parameter(Mandatory = $true)][string]$Root,
    [switch]$NoRebootDelete
  )

  return [PSCustomObject]@{
    Path = $Path
    Root = $Root
    NoRebootDelete = [bool]$NoRebootDelete
  }
}

# --- KI-13 ③：被锁文件的有限重试 + 重启删除登记 -------------------------------
# 语义分层（用户裁定 1/3，T6 同路径重装保护）：
#   立即删除成功                       → 成功；
#   重试后成功                         → 成功；
#   仍锁但移到唯一隔离路径并登记成功   → 成功（摘要行报告 scheduled 计数）；
#   无法隔离或登记失败                 → 硬失败（cleanupErrors → 非零退出）。

function Invoke-RebootDeleteRegistration {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Reason
  )

  $isolated = $false
  foreach ($root in $script:isolatedCleanupRoots) {
    if (Test-PathWithinRoot -Path $Path -Root $root) {
      $isolated = $true
      break
    }
  }
  if (-not $isolated) {
    Add-CleanupError -Path $Path -Reason 'reboot deletion refused outside a unique cleanup quarantine'
    return $false
  }
  if ([ExvCleanup.NativeMethods]::MoveFileEx($Path, $null, $script:MoveFileDelayUntilReboot)) {
    $script:scheduledRebootDeletes++
    Write-Host ("已登记重启删除（部分文件将在重启后自动删除）：" + $Path)
    return $true
  }
  $win32 = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
  Add-CleanupError -Path $Path -Reason ("reboot delete registration failed (Win32=$win32); reason: $Reason")
  return $false
}

function Move-ToUniqueCleanupQuarantine {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Reason
  )

  # Keep the isolated path beside its EXV-owned source, on the same volume.
  # A fresh GUID prevents a later installation from reusing the queued path.
  $parent = Split-Path -Parent (Get-FullPath $Path)
  Assert-PathUnderRoot -Path $Path -Root $parent
  for ($attempt = 0; $attempt -lt 8; $attempt++) {
    $candidate = Join-Path $parent ('.exv-uninstall-' + [guid]::NewGuid().ToString('N'))
    Assert-PathUnderRoot -Path $candidate -Root $parent
    if (Test-Path -LiteralPath $candidate) { continue }
    try {
      Move-Item -LiteralPath $Path -Destination $candidate -ErrorAction Stop
      if ((Test-Path -LiteralPath $Path) -or -not (Test-Path -LiteralPath $candidate)) {
        throw 'quarantine move did not leave exactly the isolated target'
      }
      $script:isolatedCleanupRoots.Add($candidate)
      return $candidate
    } catch {
      Add-CleanupError -Path $Path -Reason ("locked target could not be isolated; no reboot deletion registered ($Reason): $($_.Exception.Message)")
      return ''
    }
  }
  Add-CleanupError -Path $Path -Reason "could not allocate unique quarantine path; no reboot deletion registered ($Reason)"
  return ''
}

function Write-CleanupItem {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Action,
    [Parameter(Mandatory = $true)][string]$Outcome
  )

  $source = 'C++-v3.3.x/Rust-current/2026-09-17-path-investigation'
  $owner = 'EXV-service'
  $category = 'runtime-or-installation'
  $preserve = $false
  if (($appRoot -and (Test-PathWithinRoot -Path $Path -Root $appRoot)) -or
      ($rustConfigDir -and (Test-PathWithinRoot -Path $Path -Root $rustConfigDir)) -or
      ($explicitConfigDir -and (Test-PathWithinRoot -Path $Path -Root $explicitConfigDir)) -or
      ($redirectedConfigDir -and (Test-PathWithinRoot -Path $Path -Root $redirectedConfigDir)) -or
      ($appWebView2Dir -and (Test-PathWithinRoot -Path $Path -Root $appWebView2Dir)) -or
      ($tauriWebView2Dir -and (Test-PathWithinRoot -Path $Path -Root $tauriWebView2Dir)) -or
      ($ownedAuthWebViewDir -and (Test-PathWithinRoot -Path $Path -Root $ownedAuthWebViewDir)) -or
      ($explicitBrowserDir -and (Test-PathWithinRoot -Path $Path -Root $explicitBrowserDir)) -or
      ($InstallDir -and (Test-PathWithinRoot -Path $Path -Root (Join-Path $InstallDir $script:installUserLeafRel)))) {
    $source = 'C++-v3.3.x/Rust-current/T4b'
    $owner = 'original-user'
    $category = 'configuration-credential-or-profile'
    $preserve = $true
  } elseif (($localResourceRoot -and (Test-PathWithinRoot -Path $Path -Root $localResourceRoot)) -or
            ($machineResourceRoot -and (Test-PathWithinRoot -Path $Path -Root $machineResourceRoot))) {
    $source = 'Rust-current/2026-09-17-path-investigation'
    $category = 'runtime-journal-or-log'
  }
  $record = [ordered]@{
    path_or_id = (Get-FullPath $Path)
    historical_source = $source
    owner = $owner
    category = $category
    preserve_on_upgrade = $preserve
    action = $Action
    outcome = $Outcome
  }
  [Console]::WriteLine('EXV-CLEANUP-ITEM: ' + (ConvertTo-Json -InputObject $record -Compress))
}

function Remove-FileWithLockHandling {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Reason,
    [switch]$NoRebootDelete,
    [switch]$Isolated
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
  if ($NoRebootDelete) {
    Add-CleanupError -Path $Path -Reason "install path is locked; no reboot deletion registered ($Reason)"
    return $false
  }
  if ($Isolated) {
    return (Invoke-RebootDeleteRegistration -Path $Path -Reason $Reason)
  }
  $quarantine = Move-ToUniqueCleanupQuarantine -Path $Path -Reason $Reason
  if (-not $quarantine) { return $false }
  return (Remove-FileWithLockHandling -Path $quarantine -Reason $Reason -Isolated)
}

function Remove-TreeWithLockHandling {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Reason,
    [switch]$NoRebootDelete,
    [switch]$Isolated
  )

  # Walk only real directories inside this target. Remove junction/symlink
  # children as links before a recursive Remove-Item, never entering targets.
  $pending = New-Object 'System.Collections.Generic.Stack[string]'
  $pending.Push($Path)
  try {
    while ($pending.Count -gt 0) {
      $directory = $pending.Pop()
      foreach ($child in (Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop)) {
        if (-not (Test-PathWithinRoot -Path $child.FullName -Root $Path)) {
          throw "tree child escaped cleanup root: $($child.FullName)"
        }
        if (($child.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
          # Windows PowerShell 5.1 Remove-Item on a junction can throw a null
          # reference; .NET Delete on the reparse node unlinks the node only.
          if ($child.PSIsContainer) {
            [IO.Directory]::Delete($child.FullName)
          } else {
            [IO.File]::Delete($child.FullName)
          }
        } elseif ($child.PSIsContainer) {
          $pending.Push($child.FullName)
        }
      }
    }
  } catch {
    Add-CleanupError -Path $Path -Reason ("tree boundary check failed; preserved: $($_.Exception.Message)")
    return $false
  }

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

  if (-not $Isolated) {
    if ($NoRebootDelete) {
      Add-CleanupError -Path $Path -Reason "install path is locked; no reboot deletion registered ($Reason)"
      return $false
    }
    $quarantine = Move-ToUniqueCleanupQuarantine -Path $Path -Reason $Reason
    if (-not $quarantine) { return $false }
    return (Remove-TreeWithLockHandling -Path $quarantine -Reason $Reason -Isolated)
  }

  # Only the unique quarantine tree reaches residual enumeration. Reject any
  # linked child before Get-ChildItem -Recurse can cross a reparse boundary.
  $pending = New-Object 'System.Collections.Generic.Stack[string]'
  $pending.Push($Path)
  try {
    while ($pending.Count -gt 0) {
      $directory = $pending.Pop()
      foreach ($child in (Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop)) {
        if (($child.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
          Add-CleanupError -Path $child.FullName -Reason 'reparse child in isolated cleanup tree; preserved'
          return $false
        }
        if ($child.PSIsContainer) { $pending.Push($child.FullName) }
      }
    }
  } catch {
    Add-CleanupError -Path $Path -Reason ("isolated tree boundary check failed: $($_.Exception.Message)")
    return $false
  }

  # 第二阶段：仅隔离树的残留清点——先逐文件（删除或登记），再按深序处理目录壳。登记顺序
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
    $null = (Remove-FileWithLockHandling -Path $file.FullName -Reason $Reason -Isolated)
  }
  foreach ($dir in $remainingDirs) {
    if (Test-Path -LiteralPath $dir.FullName) {
      if (-not (Remove-FileWithLockHandling -Path $dir.FullName -Reason "$Reason (directory shell)" -Isolated)) {
        # 删除或登记失败已由 Remove-FileWithLockHandling 计错；继续处理兄弟目录。
      }
    }
  }
  if (Test-Path -LiteralPath $Path) {
    return (Remove-FileWithLockHandling -Path $Path -Reason "$Reason (tree root)" -Isolated)
  }
  return $true
}

function Remove-ConfigTarget {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root,
    [switch]$NoRebootDelete
  )

  Assert-PathUnderRoot -Path $Path -Root $Root
  if (-not (Test-Path -LiteralPath $Path)) {
    Write-Host ((Zh '5pyq5om+5Yiw77ya') + $Path)
    return
  }

  if ($PSCmdlet.ShouldProcess($Path, (Zh '5Yig6ZmkIEVYViDmnKzlnLDnlKjmiLfphY3nva7mlofku7Y='))) {
    if (Remove-FileWithLockHandling -Path $Path -Reason 'single-file cleanup' -NoRebootDelete:$NoRebootDelete) {
      Write-Host ((Zh '5bey5Yig6Zmk77ya') + $Path)
    }
  }
}

function Remove-ConfigTree {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root,
    [switch]$NoRebootDelete
  )

  Assert-PathUnderRoot -Path $Path -Root $Root
  if (-not (Test-Path -LiteralPath $Path)) {
    Write-Host ((Zh '5pyq5om+5Yiw77ya') + $Path)
    return
  }

  if ($PSCmdlet.ShouldProcess($Path, (Zh '5Yig6ZmkIEVYViDmnKzlnLDnlKjmiLfphY3nva7nm67lvZU='))) {
    if (Remove-TreeWithLockHandling -Path $Path -Reason 'tree cleanup' -NoRebootDelete:$NoRebootDelete) {
      Write-Host ((Zh '5bey5Yig6Zmk77ya') + $Path)
    }
  }
}

function Remove-ConfigPath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root,
    [switch]$NoRebootDelete
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

  Remove-ConfigTarget -Path $Path -Root $Root -NoRebootDelete:$NoRebootDelete
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

# --- S3c：install_manifest.tsv 清单驱动目标集（单一真相源） --------------------
# 清单 schema、根族语义与 -ManifestPath 探测约定见清单头注释
# （src\platform\win32\windows_setup_rust\install_manifest.tsv，打包态随包注入 support\）。
# 本脚本消费：configdir-leaf/pattern（anchor=configdir、lockdir）行 → 自定义/重定向
# 配置根已知叶；userprofile/localappdata/programdata/temp 根族行 → 数据根目标；
# install-legacy-3parent 行 → 历代安装根名称；installdir + delete_u1_only 行 →
# 安装根内用户数据（exv-ui.exe.WebView2 UDF）。installdir 其余行（载荷/支撑/历代
# 载荷，action=delete_always）归卸载器引擎，本脚本不消费；action=preserve 行
# （如 Helper 安全隔离目录）仅登记备查，永不主动删除。

function Get-InstallManifestPath {
  if ($ManifestPath) {
    if (-not (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
      throw "install manifest not found (-ManifestPath): $ManifestPath"
    }
    return (Get-FullPath $ManifestPath)
  }
  # S3a 约定：先随包（本脚本与清单同在 support\ 下），后仓内（仓库根源码路径）。
  $candidates = @(
    (Join-Path $PSScriptRoot 'install_manifest.tsv'),
    (Join-Path (Split-Path -Parent $PSScriptRoot) 'src\platform\win32\windows_setup_rust\install_manifest.tsv')
  )
  foreach ($candidate in $candidates) {
    if (-not [string]::IsNullOrEmpty($candidate) -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
      return (Get-FullPath $candidate)
    }
  }
  throw ('install manifest missing (fail-closed): 未找到 install_manifest.tsv。' +
    '请以 -ManifestPath 显式传入清单路径；随包形态应与本脚本同置于 support\ 目录，' +
    '仓内/测试形态应位于仓库根 src\platform\win32\windows_setup_rust\install_manifest.tsv。' +
    '清单是卸载清理目标的唯一真相源，缺失即拒绝执行清理。')
}

function Read-InstallManifest {
  param(
    [Parameter(Mandatory = $true)][string]$Path
  )

  $expectedHeader = "scope`tpath`tclass`tintroduced`tremoved`tpipeline`tnoreboot`taction`tpurpose"
  $rows = @()
  $headerSeen = $false
  foreach ($rawLine in (Get-Content -LiteralPath $Path -Encoding UTF8)) {
    $line = "$rawLine".TrimEnd("`r")
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    if ($line.StartsWith('#')) { continue }
    if (-not $headerSeen) {
      if ($line -ne $expectedHeader) {
        throw "install manifest header mismatch: $Path"
      }
      $headerSeen = $true
      continue
    }
    $fields = $line -split "`t", -1
    if ($fields.Count -ne 9) {
      throw "install manifest row must have exactly 9 columns ($Path): $line"
    }
    $rows += [pscustomobject]@{
      Scope = $fields[0]
      Path = $fields[1]
      Class = $fields[2]
      Introduced = $fields[3]
      Removed = $fields[4]
      Pipeline = $fields[5]
      NoReboot = $fields[6]
      Action = $fields[7]
      Purpose = $fields[8]
    }
  }
  if (-not $headerSeen) { throw "install manifest has no header row: $Path" }
  if ($rows.Count -lt 1) { throw "install manifest has no data rows: $Path" }
  return $rows
}

function Get-ManifestRow {
  # 按「scope|path」键取清单行；缺失或重复都是清单不完整，fail-closed 硬失败。
  param(
    [Parameter(Mandatory = $true)][string]$Key
  )

  $separator = $Key.IndexOf('|')
  if ($separator -lt 1) {
    throw "invalid manifest anchor key: $Key"
  }
  $scope = $Key.Substring(0, $separator)
  $rowPath = $Key.Substring($separator + 1)
  $matched = @($script:manifestRows | Where-Object { $_.Scope -eq $scope -and $_.Path -eq $rowPath })
  if ($matched.Count -ne 1) {
    throw "install manifest anchor row missing or duplicated (fail-closed): $Key"
  }
  return $matched[0]
}

function Get-ScopedTargetRoot {
  # 断言根收紧（运行期护栏）：目标路径一律按根族根解析（Join-Path <根族根> <path>），
  # 仅 New-CleanupEntry 的 Root 断言按本函数收紧——programdata 的 EXV\ 前缀行断言根
  # 收紧为 ProgramData\EXV（exv/EXV 同一物理根，只动 Helper 子树）；localappdata 的
  # Programs\EXV\ 前缀行断言根收紧为 %LOCALAPPDATA%\Programs\EXV（Programs 下只认
  # EXV 自己的子树）。temp 根族由调用方双根展开，不经本函数。
  param(
    [Parameter(Mandatory = $true)][string]$Scope,
    [Parameter(Mandatory = $true)][string]$RowPath
  )

  switch ($Scope) {
    'userprofile' { return $userProfile }
    'roamingappdata' { return $roamingAppData }
    'localappdata' {
      if ($RowPath.StartsWith('Programs\EXV\')) { return $programsRoot }
      return $localAppData
    }
    'programdata' {
      if ($RowPath.StartsWith('EXV\')) { return (Join-Path $programData 'EXV') }
      return $programData
    }
    default { throw "unsupported manifest root scope for target generation: $Scope" }
  }
}

function Add-ManifestConfigTargets {
  # 自定义/重定向配置根的已知 EXV 叶从清单 configdir-leaf 与 pattern 行生成。
  # 自定义配置根可混放第三方文件，只治理清单已知 EXV 叶——删除管线全程
  # -LiteralPath，通配只能经枚举展开（模式叶），不用宽泛 *.tmp 或前缀整树删除。
  param(
    [Parameter(Mandatory = $true)][ref]$PathTargets,
    [Parameter(Mandatory = $true)][ref]$TreeTargets,
    [Parameter(Mandatory = $true)][ref]$EmptyDirTargets,
    [Parameter(Mandatory = $true)][string]$ConfigDir,
    [Parameter(Mandatory = $true)][string]$Root
  )

  # 固定行（configdir-leaf）：path→逐文件、tree→整树、empty-dir→目录壳。
  # 锁目录壳行（存在同级 <dir>\<owner 叶> 属主行的 empty-dir 行）计入锁目录列表，
  # 由下方锁目录规则统一展开内部（含目录壳，深序先于配置根壳）；「.」行
  # （preserve_with_shell）是配置根本身：内容一律保留，仅在删尽已知叶后目录为
  # 空时移除目录壳——壳登记放在最后，保证锁目录壳先行清空。
  $lockDirs = @()
  $configRootShell = $false
  foreach ($row in $script:manifestConfigLeafRows) {
    if ($row.Action -eq 'preserve') { continue }
    if ($row.Path -eq '.') {
      $configRootShell = $true
      continue
    }
    $targetPath = Join-Path $ConfigDir $row.Path
    if ($row.Pipeline -eq 'empty-dir') {
      $ownerRowPath = $row.Path + '\' + $script:manifestLockOwnerLeaf
      if ($script:manifestLockOwnerLeaf -and
          (@($script:manifestConfigLeafRows | Where-Object { $_.Path -eq $ownerRowPath }).Count -eq 1)) {
        $lockDirs += $targetPath
      } else {
        $EmptyDirTargets.Value += (New-CleanupEntry -Path $targetPath -Root $Root)
      }
      continue
    }
    if ($row.Pipeline -eq 'tree') {
      $TreeTargets.Value += (New-CleanupEntry -Path $targetPath -Root $Root -NoRebootDelete:($row.NoReboot -eq '1'))
    } else {
      $PathTargets.Value += (New-CleanupEntry -Path $targetPath -Root $Root -NoRebootDelete:($row.NoReboot -eq '1'))
    }
  }
  # 模式叶（anchor=configdir）：枚举配置根直接子项，regex 匹配 basename。
  try {
    Assert-PathUnderRoot -Path $ConfigDir -Root $Root
    if (Test-Path -LiteralPath $ConfigDir -PathType Container) {
      foreach ($child in (Get-ChildItem -LiteralPath $ConfigDir -Force)) {
        foreach ($patternRow in $script:manifestConfigPatternRows) {
          if ($child.Name -notmatch $patternRow.Regex) { continue }
          if (-not $child.PSIsContainer -and $patternRow.Pipeline -eq 'path') {
            $PathTargets.Value += (New-CleanupEntry -Path $child.FullName -Root $Root)
          } elseif ($child.PSIsContainer -and $patternRow.Pipeline -eq 'empty-dir') {
            # 临时锁目录：内部属主叶按锁目录规则展开，随后仅空时移除目录壳。
            $lockDirs += $child.FullName
          }
        }
      }
    }
    foreach ($lockDir in $lockDirs) {
      Assert-PathUnderRoot -Path $lockDir -Root $Root
      $PathTargets.Value += (New-CleanupEntry -Path (Join-Path $lockDir $script:manifestLockOwnerLeaf) -Root $Root)
      if (Test-Path -LiteralPath $lockDir -PathType Container) {
        foreach ($child in (Get-ChildItem -LiteralPath $lockDir -Force -File)) {
          foreach ($patternRow in $script:manifestLockPatternRows) {
            if ($child.Name -match $patternRow.Regex) {
              $PathTargets.Value += (New-CleanupEntry -Path $child.FullName -Root $Root)
            }
          }
        }
      }
      $EmptyDirTargets.Value += (New-CleanupEntry -Path $lockDir -Root $Root)
    }
  } catch {
    Add-CleanupError -Path $ConfigDir -Reason $_.Exception.Message
  }
  if ($configRootShell) {
    $EmptyDirTargets.Value += (New-CleanupEntry -Path $ConfigDir -Root $Root)
  }
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

    $wasPresent = Test-Path -LiteralPath $entry.Path
    $errorsBefore = $cleanupErrors.Count
    $scheduledBefore = $script:scheduledRebootDeletes
    $action = if ($WhatIfPreference) { 'preview_remove' } elseif ($EmptyDirectory) {
      'remove_if_empty'
    } else { 'remove' }
    $outcome = if ($wasPresent) { 'found' } else { 'missing' }
    try {
      if ($EmptyDirectory) {
        Remove-EmptyDirectoryIfExists -Path $entry.Path -Root $entry.Root
      } elseif ($Tree) {
        Remove-ConfigTree -Path $entry.Path -Root $entry.Root -NoRebootDelete:$entry.NoRebootDelete
      } else {
        Remove-ConfigPath -Path $entry.Path -Root $entry.Root -NoRebootDelete:$entry.NoRebootDelete
      }
    } catch {
      Add-CleanupError -Path $entry.Path -Reason $_.Exception.Message
    } finally {
      if (-not $WhatIfPreference -and $wasPresent) {
        if ($cleanupErrors.Count -gt $errorsBefore) {
          $outcome = 'failed'
        } elseif ($script:scheduledRebootDeletes -gt $scheduledBefore) {
          $outcome = 'scheduled_isolated'
        } elseif (-not (Test-Path -LiteralPath $entry.Path)) {
          $outcome = 'removed'
        } else {
          $outcome = 'retained_nonempty'
        }
      }
      Write-CleanupItem -Path $entry.Path -Action $action -Outcome $outcome
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
      'RoamingAppDataRoot', 'ConfigDir', 'BrowserDataDir', 'InstallDir', 'TempRoot', 'SystemTempRoot',
      'ManifestPath')) {
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
$explicitBrowserDir = if ($PSBoundParameters.ContainsKey('BrowserDataDir')) { $BrowserDataDir } elseif ($UserProfileRoot) {
  ''
} else { $env:EXV_ATRUST_BROWSER_DATA_DIR }
$programData = Get-ProgramDataRoot
$userTemp = if ($TempRoot) { $TempRoot } elseif ($UserProfileRoot) {
  Join-Path $localAppData 'Temp'
} else { [IO.Path]::GetTempPath() }
$systemTemp = if ($SystemTempRoot) { $SystemTempRoot } else {
  Join-Path ([Environment]::GetFolderPath('Windows')) 'Temp'
}
# --- S3c：加载清单（fail-closed）并推导分类锚 ---------------------------------
# 清单是清理目标的唯一真相源；以下锚行键同时是脚本与清单的契约——锚行缺失即
# 清单不完整，硬失败（Get-ManifestRow）。Write-CleanupItem 的分类桶（用户配置/
# journal/默认）按这些锚根做包含判定，属运行时控制流。
$script:manifestRows = Read-InstallManifest -Path (Get-InstallManifestPath)
$appRoot = Join-Path $localAppData (Get-ManifestRow 'localappdata|EXV').Path
$redirectPath = Join-Path $appRoot 'profile.redirect'
$appWebView2Dir = Join-Path $localAppData (Get-ManifestRow 'localappdata|Programs\EXV\exv-ui.exe.WebView2').Path
$programsRoot = Split-Path -Parent $appWebView2Dir
# Tauri identifier 命名的 WebView2 用户数据根（identifier 见
# tauri/app/tauri.conf.json，改名须同步清单 localappdata|com.exv.vpn.desktop 行）：
# EBWebView（前端外观域 localStorage——主题/完整极简模式持久化于此）与
# tauri-plugin-log 的 logs 都在其下。2026-10-10 实录：该目录不在卸载清理范围时，
# localStorage 跨卸载存活，重装后与「卸载即重置的快速入门已见标记」错配——极简
# 小窗内溢出无极简适配的快速入门表单。按 EXV 专属根整树治理（同 KI-13 ⑤ 语义，
# 不逐叶列举）。
$tauriWebView2Dir = Join-Path $localAppData (Get-ManifestRow 'localappdata|com.exv.vpn.desktop').Path
$ownedAuthWebViewDir = Join-Path $roamingAppData (Get-ManifestRow 'roamingappdata|EXV\atrust-webview').Path
$rustConfigDir = if ($userProfile) { Join-Path $userProfile (Get-ManifestRow 'userprofile|.exv').Path } else { '' }
$localResourceRoot = Join-Path $localAppData (Get-ManifestRow 'localappdata|ExvVpn').Path
$machineResourceRoot = Join-Path $programData (Get-ManifestRow 'programdata|ExvVpn').Path
# 安装根内用户数据（installdir + delete_u1_only；现役=exv-ui.exe.WebView2 UDF）。
$script:installUserLeafRows = @($script:manifestRows |
  Where-Object { $_.Scope -eq 'installdir' -and $_.Action -eq 'delete_u1_only' })
if ($script:installUserLeafRows.Count -lt 1) {
  throw 'install manifest missing installdir delete_u1_only row (fail-closed): exv-ui.exe.WebView2'
}
$script:installUserLeafRel = $script:installUserLeafRows[0].Path

# 配置根已知叶行缓存 + 模式叶预解析（anchor=configdir/lockdir）+ 锁目录属主叶推导。
$script:manifestConfigLeafRows = @($script:manifestRows | Where-Object { $_.Scope -eq 'configdir-leaf' })
$script:manifestConfigPatternRows = @($script:manifestRows |
  Where-Object { $_.Scope -eq 'pattern' -and $_.Path.StartsWith('configdir:') -and $_.Action -ne 'preserve' } |
  ForEach-Object {
    [pscustomobject]@{ Regex = $_.Path.Substring('configdir:'.Length); Pipeline = $_.Pipeline }
  })
$script:manifestLockPatternRows = @($script:manifestRows |
  Where-Object { $_.Scope -eq 'pattern' -and $_.Path.StartsWith('lockdir:') -and $_.Action -ne 'preserve' } |
  ForEach-Object {
    [pscustomobject]@{ Regex = $_.Path.Substring('lockdir:'.Length) }
  })
# 锁目录壳行 = configdir-leaf 的 empty-dir 行；其属主叶行 = 同级 <dir>\<leaf> 的
# path 行。属主叶名由该结构推导（现值 owner.json），临时锁目录按同一规则展开。
$script:manifestLockDirRelPaths = @($script:manifestConfigLeafRows |
  Where-Object { $_.Pipeline -eq 'empty-dir' -and $_.Path -ne '.' } |
  ForEach-Object { $_.Path })
$lockOwnerLeaves = @($script:manifestConfigLeafRows |
  Where-Object { $_.Pipeline -eq 'path' -and ($script:manifestLockDirRelPaths -contains (Split-Path -Parent $_.Path)) } |
  ForEach-Object { Split-Path -Leaf $_.Path } |
  Sort-Object -Unique)
$script:manifestLockOwnerLeaf = if ($lockOwnerLeaves.Count -eq 1) { $lockOwnerLeaves[0] } else { '' }

$pathTargets = @()
$treeTargets = @()
$emptyDirTargets = @()

# KI-13 ⑤ + S3c：清单治理——目标集从 install_manifest.tsv 生成（单一真相源，防
# 三处手写清单漂移）。EXV 专属根按「代码所有权」整树删除，不再按猜文件名逐叶
# 列举：%LOCALAPPDATA%\EXV 是产品固定命名空间（profile\default、
# ui-preferences.json、profile.redirect、旧 Helper、历代 WebView2 状态都在其中），
# 整树删除天然覆盖所有历代漂移叶；逐叶清单只保留给「自定义/重定向配置目录」
# （那里可能有第三方文件，只能删已知 EXV 叶）。留在代码的运行时控制流：
# action 列承载的 U1/U2 门控、历代安装根对当前安装根（及其后代）的排除、
# quarantine 隔离命名域、锁重试/隔离/PFRO 登记管线。

# KI-13 ④：PreserveUserData（升级语义）跳过用户数据段——连接配置（~/.exv）、
# UI 偏好与 profile（%LOCALAPPDATA%\EXV）、凭据管理器、自定义/重定向配置目录、
# 历代安装根（可能含 WebView2 用户数据）。缓存/账本/服务态照常清理。
$redirectedConfigDir = ''
if (-not $PreserveUserData) {
  # 用户数据树：清单根族行（userprofile/localappdata，action=delete_u1_only）。
  # The exact roots are owned by EXV.  This is intentionally root-level cleanup
  # after the user opted in, so unknown future state does not survive a
  # reinstall merely because it was not in a file-by-file allow list.
  foreach ($row in @($script:manifestRows | Where-Object {
      $_.Scope -in @('userprofile', 'localappdata', 'roamingappdata') -and $_.Action -eq 'delete_u1_only' })) {
    if ($row.Scope -eq 'userprofile' -and -not $userProfile) { continue }
    $scopeRoot = if ($row.Scope -eq 'userprofile') { $userProfile } elseif ($row.Scope -eq 'roamingappdata') { $roamingAppData } else { $localAppData }
    $assertRoot = Get-ScopedTargetRoot -Scope $row.Scope -RowPath $row.Path
    $treeTargets += (New-CleanupEntry -Path (Join-Path $scopeRoot $row.Path) -Root $assertRoot -NoRebootDelete:($row.NoReboot -eq '1'))
  }
} else {
  Write-Host "PreserveUserData: skip user-data trees ($appRoot, $rustConfigDir, $tauriWebView2Dir, credentials)"
  foreach ($retained in @($appRoot, $rustConfigDir, $appWebView2Dir, $tauriWebView2Dir, $ownedAuthWebViewDir)) {
    if ($retained) {
      $presence = if (Test-Path -LiteralPath $retained) { 'preserved' } else { 'missing' }
      Write-CleanupItem -Path $retained -Action 'preserve' -Outcome $presence
    }
  }
  if ($explicitConfigDir) {
    $presence = if (Test-Path -LiteralPath $explicitConfigDir) { 'preserved' } else { 'missing' }
    Write-CleanupItem -Path $explicitConfigDir -Action 'preserve' -Outcome $presence
  }
  if ($explicitBrowserDir) {
    $presence = if (Test-Path -LiteralPath $explicitBrowserDir) { 'preserved' } else { 'missing' }
    Write-CleanupItem -Path $explicitBrowserDir -Action 'preserve' -Outcome $presence
  }
  if ($InstallDir) {
    try {
      foreach ($row in $script:installUserLeafRows) {
        $installedWebView2 = Join-Path (Get-FullPath $InstallDir) $row.Path
        $presence = if (Test-Path -LiteralPath $installedWebView2) { 'preserved' } else { 'missing' }
        Write-CleanupItem -Path $installedWebView2 -Action 'preserve' -Outcome $presence
      }
    } catch {
      Add-CleanupError -Path $InstallDir -Reason $_.Exception.Message
    }
  }
}

# 显式浏览器根可能混放其他文件；只消费清单 browserdir 模式命中的 EXV 资料。
if ($explicitBrowserDir -and -not $PreserveUserData) {
  try {
    Assert-ConfigDirSafeForCleanup -Path $explicitBrowserDir -SharedRoots @(
      $userProfile, $localAppData, $roamingAppData, $programData, $userTemp, $systemTemp)
    $browserRoot = Get-FullPath $explicitBrowserDir
    Assert-PathUnderRoot -Path $browserRoot -Root $browserRoot
    $browserPatterns = @($script:manifestRows | Where-Object {
      $_.Scope -eq 'pattern' -and $_.Path.StartsWith('browserdir:') -and $_.Action -eq 'delete_u1_only' })
    if (Test-Path -LiteralPath $browserRoot -PathType Container) {
      foreach ($child in (Get-ChildItem -LiteralPath $browserRoot -Force)) {
        foreach ($row in $browserPatterns) {
          if ($child.Name -notmatch $row.Path.Substring('browserdir:'.Length)) { continue }
          $entry = New-CleanupEntry -Path $child.FullName -Root $browserRoot -NoRebootDelete:($row.NoReboot -eq '1')
          if ($child.PSIsContainer -and $row.Pipeline -eq 'tree') { $treeTargets += $entry }
          elseif (-not $child.PSIsContainer -and $row.Pipeline -eq 'path') { $pathTargets += $entry }
        }
      }
    }
  } catch {
    Add-CleanupError -Path $explicitBrowserDir -Reason $_.Exception.Message
  }
}

# 机器数据（缓存/账本/服务态，action=delete_always）：U1 全新卸载与 U2 保留升级
# 语义下都清，无 PreserveUserData 门。temp 根族行展开为用户/系统临时根两处。
foreach ($row in @($script:manifestRows | Where-Object {
    $_.Scope -in @('userprofile', 'localappdata', 'programdata', 'temp') -and $_.Action -eq 'delete_always' })) {
  if ($row.Scope -eq 'userprofile' -and -not $userProfile) { continue }
  $assertRoot = $null
  if ($row.Scope -eq 'temp') {
    $rowRoots = @($userTemp, $systemTemp)
  } else {
    $scopeRoot = if ($row.Scope -eq 'userprofile') { $userProfile } elseif ($row.Scope -eq 'localappdata') {
      $localAppData
    } else { $programData }
    $rowRoots = @($scopeRoot)
    $assertRoot = Get-ScopedTargetRoot -Scope $row.Scope -RowPath $row.Path
  }
  foreach ($rowRoot in $rowRoots) {
    $entryRoot = if ($null -ne $assertRoot) { $assertRoot } else { $rowRoot }
    $entry = New-CleanupEntry -Path (Join-Path $rowRoot $row.Path) -Root $entryRoot -NoRebootDelete:($row.NoReboot -eq '1')
    if ($row.Pipeline -eq 'tree') {
      $treeTargets += $entry
    } elseif ($row.Pipeline -eq 'path') {
      $pathTargets += $entry
    } else {
      $emptyDirTargets += $entry
    }
  }
}

# 安装根内用户数据（installdir + delete_u1_only，现役=exv-ui.exe.WebView2 UDF）：
# 升级保留、全新卸载删除；禁重启删除登记（noreboot=1，同路径重装保护）。
if ($InstallDir -and -not $PreserveUserData) {
  $installRoot = Get-FullPath $InstallDir
  foreach ($row in $script:installUserLeafRows) {
    $treeTargets += (New-CleanupEntry -Path (Join-Path $installRoot $row.Path) -Root $installRoot -NoRebootDelete:($row.NoReboot -eq '1'))
  }
}

# KI-13 ⑥ 历代安装根漂移治理：安装名来自清单 install-legacy-3parent 行（ECNU
# VPN/ECNU-VPN 时代的 per-user 安装位与 C++ 时代的 Program Files 安装位，考古见
# scripts/clear-windows-legacy-state.ps1）。3 父目录推导与当前安装根（及其后代）
# 的排除是运行时控制流：仅在卸载链上下文（显式 -InstallDir）且非保留语义时整树
# 清理；当前安装目录交给卸载器引擎处理，脚本侧绝不触碰——否则会连同活动卸载器
# 映像一起登记重启删除，破坏同路径重装安全性。
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
  $legacyInstallNames = @($script:manifestRows |
    Where-Object { $_.Scope -eq 'install-legacy-3parent' -and $_.Action -ne 'preserve' } |
    ForEach-Object { $_.Path })
  foreach ($pair in $legacyInstallParents) {
    $base = $pair[0]
    $root = $pair[1]
    foreach ($name in $legacyInstallNames) {
      $candidate = Join-Path $base $name
      $candidateNorm = Normalize-FullPath $candidate
      if ($candidateNorm.Equals($currentInstallRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        continue
      }
      if ($currentInstallRoot.StartsWith($candidateNorm + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
        continue
      }
      $treeTargets += (New-CleanupEntry -Path $candidate -Root $root -NoRebootDelete)
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
    Add-ManifestConfigTargets -PathTargets ([ref]$pathTargets) `
      -TreeTargets ([ref]$treeTargets) -EmptyDirTargets ([ref]$emptyDirTargets) `
      -ConfigDir $customConfigDir -Root $customConfigDir
  } catch {
    Add-CleanupError -Path $explicitConfigDir -Reason $_.Exception.Message
  }
}

# profile.redirect 的重定向目标（C++ 时代配置根）：必须在 %LOCALAPPDATA%\EXV
# 整树删除之前只读解析；保留语义仅记录已校验目标，不列入删除清单。
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
    if ($PreserveUserData) {
      $presence = if (Test-Path -LiteralPath $redirectedConfigDir) { 'preserved' } else { 'missing' }
      Write-CleanupItem -Path $redirectedConfigDir -Action 'preserve' -Outcome $presence
    } else {
      Add-ManifestConfigTargets -PathTargets ([ref]$pathTargets) `
        -TreeTargets ([ref]$treeTargets) `
        -EmptyDirTargets ([ref]$emptyDirTargets) `
        -ConfigDir $redirectedConfigDir `
        -Root $redirectedConfigDir
    }
  } catch {
    Add-CleanupError -Path $redirectedConfigDir -Reason $_.Exception.Message
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

# 即使有其它清理项硬失败，先报告已隔离并成功登记的数量；调用方必须显示
# 待重启事实，不能因为脚本 exit 非零而丢失该摘要。
[Console]::WriteLine("EXV-CLEANUP-SUMMARY: scheduled=$scheduledRebootDeletes")

if ($cleanupErrors.Count -gt 0) {
  # KI-13 ③：只有硬失败（重试与重启删除登记都做不到）才非零退出。
  throw ("EXV cleanup incomplete ({0} errors):`n{1}" -f $cleanupErrors.Count, ($cleanupErrors -join "`n"))
}
