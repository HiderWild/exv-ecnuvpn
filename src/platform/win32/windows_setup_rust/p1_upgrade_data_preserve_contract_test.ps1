# KI-13 ④⑤⑥ 契约测试：升级保留语义、被锁文件降级、历代安装根治理。
# 全部在临时沙箱根内执行真实脚本；不触碰真实安装/服务/用户目录。
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$setupRoot = Split-Path -Parent $PSCommandPath
$repoRoot = $setupRoot
1..4 | ForEach-Object { $repoRoot = Split-Path -Parent $repoRoot }
$cleanupScript = Join-Path $repoRoot 'scripts\clear-local-user-config.ps1'

function Assert-Contains([string]$Text, [string]$Pattern, [string]$Message) {
  if ($Text -notmatch $Pattern) {
    throw "EXPECT FAILED: $Message; pattern=$Pattern"
  }
}

function Assert-PathMissing([string]$Path, [string]$Message) {
  if (Test-Path -LiteralPath $Path) {
    throw "EXPECT FAILED: $Message ($Path)"
  }
}

function Assert-PathPresent([string]$Path, [string]$Message) {
  if (-not (Test-Path -LiteralPath $Path)) {
    throw "EXPECT FAILED: $Message ($Path)"
  }
}

function New-Sandbox {
  $testRoot = Join-Path ([IO.Path]::GetTempPath()) ("exv-ki13-preserve-{0}" -f [guid]::NewGuid())
  $sandbox = [PSCustomObject]@{
    Root = $testRoot
    LocalAppData = Join-Path $testRoot 'LocalAppData'
    UserProfile = Join-Path $testRoot 'UserProfile'
    ProgramData = Join-Path $testRoot 'ProgramData'
    Temp = Join-Path $testRoot 'Temp'
    SystemTemp = Join-Path $testRoot 'SystemTemp'
    ProgramFiles = Join-Path $testRoot 'ProgramFiles'
  }
  foreach ($dir in @($sandbox.LocalAppData, $sandbox.UserProfile, $sandbox.ProgramData,
      $sandbox.Temp, $sandbox.SystemTemp, $sandbox.ProgramFiles)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
  }
  return $sandbox
}

function New-Marker([string]$Path) {
  New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
  Set-Content -LiteralPath $Path -Value 'EXV test state' -NoNewline
}

function Invoke-CleanupScript([hashtable]$Arguments, [switch]$PreserveUserData) {
  # 以子进程运行清理脚本：可靠捕获退出码与 stdout 摘要行（生产卸载器同构路径）。
  $parts = @(
    'powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
    ('"{0}"' -f $cleanupScript), '-Force', '-NoProcessStop'
  )
  foreach ($entry in $Arguments.GetEnumerator()) {
    $parts += ('-{0}' -f $entry.Key)
    $parts += ('"{0}"' -f $entry.Value)
  }
  if ($PreserveUserData) {
    $parts += '-PreserveUserData'
  }
  $command = $parts -join ' '
  # 子进程 stderr（硬失败时的 Write-Warning 流）与 EAP=Stop 组合会把 NativeCommandError
  # 升级为终止错误；这里局部放宽以同时收集 stdout/stderr 与退出码。
  $previousEap = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try {
    $out = & powershell.exe -NoProfile -Command $command 2>&1
  } finally {
    $ErrorActionPreference = $previousEap
  }
  $code = $LASTEXITCODE
  return [PSCustomObject]@{ ExitCode = $code; Output = ($out -join "`n") }
}

function Get-ScheduledSummary([string]$Output) {
  if ($Output -match 'EXV-CLEANUP-SUMMARY:\s*scheduled=(\d+)') {
    return [int]$Matches[1]
  }
  return $null
}

# 源码契约（S3c 清单驱动后）：历代安装根名称来自 install_manifest.tsv 的
# install-legacy-3parent 行（锚定消费），3 父目录推导仍覆盖 per-user Programs 与
# 两个 Program Files 基座。powershell 子进程启动时会自动重置 ProgramFiles
# 环境变量，沙箱无法注入执行验证；Program Files 基座以源码断言覆盖，执行覆盖由
# %LOCALAPPDATA%\Programs 基座承担（同一条推导循环）。
$cleanupSource = Get-Content -LiteralPath $cleanupScript -Raw
Assert-Contains $cleanupSource 'env:ProgramFiles' 'cleanup derives legacy roots from ProgramFiles'
Assert-Contains $cleanupSource 'ProgramFiles\(x86\)' 'cleanup derives legacy roots from ProgramFiles(x86)'
Assert-Contains $cleanupSource 'install-legacy-3parent' 'cleanup consumes manifest install-legacy-3parent rows for legacy install root names'
$manifestTsv = Join-Path $repoRoot 'src\platform\win32\windows_setup_rust\install_manifest.tsv'
$manifestDataRows = @(
  Get-Content -LiteralPath $manifestTsv -Encoding UTF8 |
    Where-Object { $_ -and -not $_.StartsWith('#') } |
    Select-Object -Skip 1
)
foreach ($legacyName in @('EXV', 'ECNU VPN', 'ECNU-VPN')) {
  $legacyRows = @($manifestDataRows | Where-Object {
    ($_ -split "`t", -1)[0] -eq 'install-legacy-3parent' -and ($_ -split "`t", -1)[1] -eq $legacyName })
  if ($legacyRows.Count -ne 1) {
    throw "EXPECT FAILED: manifest must carry exactly one install-legacy-3parent row for '$legacyName'"
  }
  if ((($legacyRows[0] -split "`t", -1)[7]) -ne 'delete_u1_only') {
    throw "EXPECT FAILED: install-legacy-3parent row for '$legacyName' must be delete_u1_only"
  }
}

# ④ 升级保留语义：用户数据树幸存，缓存/账本/服务态照清，历代安装根不清理。
$sandbox = New-Sandbox
try {
  $userDataMarkers = @(
    (Join-Path $sandbox.UserProfile '.exv\config.json'),
    (Join-Path $sandbox.UserProfile '.exv\key.bin'),
    (Join-Path $sandbox.LocalAppData 'EXV\profile\default\ui-preferences.json'),
    (Join-Path $sandbox.LocalAppData 'com.exv.vpn.desktop\EBWebView\Default\Local Storage\leveldb\000003.log')
  )
  $cacheMarkers = @(
    (Join-Path $sandbox.LocalAppData 'ExvVpn\journal\journal.bin'),
    (Join-Path $sandbox.ProgramData 'ExvVpn\journal\journal.bin'),
    (Join-Path $sandbox.ProgramData 'exv\service.key'),
    (Join-Path $sandbox.ProgramData 'EXV\Helper\exv-helper.exe'),
    (Join-Path $sandbox.ProgramData 'exv-helper-session.json')
  )
  foreach ($marker in ($userDataMarkers + $cacheMarkers)) { New-Marker $marker }
  New-Marker (Join-Path $sandbox.LocalAppData 'Programs\ECNU VPN\legacy-app.bin')
  New-Marker (Join-Path $sandbox.LocalAppData 'Programs\EXV\current-install-marker.bin')

  $oldUserProfile = $env:USERPROFILE
  $oldLocalAppData = $env:LOCALAPPDATA
  $oldProgramData = $env:ProgramData
  $oldProgramFiles = $env:ProgramFiles
  $env:USERPROFILE = $sandbox.UserProfile
  $env:LOCALAPPDATA = $sandbox.LocalAppData
  $env:ProgramData = $sandbox.ProgramData
  $env:ProgramFiles = $sandbox.ProgramFiles

  $run = Invoke-CleanupScript @{
    InstallDir = (Join-Path $sandbox.LocalAppData 'Programs\EXV')
    LocalAppDataRoot = $sandbox.LocalAppData
    ProgramDataRoot = $sandbox.ProgramData
    UserProfileRoot = $sandbox.UserProfile
    TempRoot = $sandbox.Temp
    SystemTempRoot = $sandbox.SystemTemp
    ManifestPath = $manifestTsv
  } -PreserveUserData
  if ($run.ExitCode -ne 0) {
    throw "EXPECT FAILED: preserve-mode cleanup reported hard failure (exit=$($run.ExitCode)); output=$($run.Output)"
  }
  foreach ($marker in $userDataMarkers) {
    Assert-PathPresent $marker 'upgrade-preserve must keep user data'
  }
  Assert-PathPresent (Join-Path $sandbox.LocalAppData 'Programs\ECNU VPN\legacy-app.bin') `
    'upgrade-preserve must keep legacy install roots (WebView2 user data)'
  foreach ($marker in $cacheMarkers) {
    Assert-PathMissing $marker 'upgrade-preserve must still clean caches/ledger/service state'
  }

  # ⑥ 历代安装根（全清语义）：ECNU 时代与 Program Files 安装位整树清理；
  # 当前安装目录本身交给卸载器引擎，脚本不触碰。
  foreach ($marker in ($userDataMarkers + $cacheMarkers)) { New-Marker $marker }
  $run2 = Invoke-CleanupScript @{
    InstallDir = (Join-Path $sandbox.LocalAppData 'Programs\EXV')
    LocalAppDataRoot = $sandbox.LocalAppData
    ProgramDataRoot = $sandbox.ProgramData
    UserProfileRoot = $sandbox.UserProfile
    TempRoot = $sandbox.Temp
    SystemTempRoot = $sandbox.SystemTemp
    ManifestPath = $manifestTsv
  }
  if ($run2.ExitCode -ne 0) {
    throw "EXPECT FAILED: full cleanup reported hard failure (exit=$($run2.ExitCode)); output=$($run2.Output)"
  }
  Assert-PathMissing (Join-Path $sandbox.LocalAppData 'Programs\ECNU VPN') 'full cleanup must remove the legacy ECNU-era install root'
  Assert-PathMissing (Join-Path $sandbox.UserProfile '.exv') 'full cleanup must remove the Rust config root'
  Assert-PathMissing (Join-Path $sandbox.LocalAppData 'EXV') 'full cleanup must remove the EXV app root'
  Assert-PathMissing (Join-Path $sandbox.LocalAppData 'com.exv.vpn.desktop') 'full cleanup must remove the Tauri identifier WebView2 root (appearance localStorage must not survive uninstall)'
  Assert-PathPresent (Join-Path $sandbox.LocalAppData 'Programs\EXV\current-install-marker.bin') `
    'cleanup script must not touch the current install dir (uninstaller engine owns it)'
  $summary = Get-ScheduledSummary $run2.Output
  if ($null -eq $summary) {
    throw 'EXPECT FAILED: cleanup output must carry the machine-readable summary line'
  }
} finally {
  $env:USERPROFILE = $oldUserProfile
  $env:LOCALAPPDATA = $oldLocalAppData
  $env:ProgramData = $oldProgramData
  $env:ProgramFiles = $oldProgramFiles
  if (Test-Path -LiteralPath $sandbox.Root) {
    Remove-Item -LiteralPath $sandbox.Root -Recurse -Force
  }
}

# ③ 被锁文件：有限重试（3×500ms ≈ 至少 1.4s）后登记重启删除；提权环境登记
# 成功 → exit 0 + scheduled>=1；非提权环境登记失败 → 硬失败 exit!=0 且文件保留。
# 两种结果都不得是「立即 fail-fast」（elapsed 钉住重试确实发生）。
$sandbox2 = New-Sandbox
try {
  $lockedTree = Join-Path $sandbox2.LocalAppData 'ExvVpn\journal'
  $lockedFile = Join-Path $lockedTree 'journal.bin'
  New-Marker $lockedFile
  $stream = [IO.File]::Open($lockedFile, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
  $oldUserProfile = $env:USERPROFILE
  $oldLocalAppData = $env:LOCALAPPDATA
  $oldProgramData = $env:ProgramData
  $env:USERPROFILE = $sandbox2.UserProfile
  $env:LOCALAPPDATA = $sandbox2.LocalAppData
  $env:ProgramData = $sandbox2.ProgramData

  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $lockedRun = Invoke-CleanupScript @{
    LocalAppDataRoot = $sandbox2.LocalAppData
    ProgramDataRoot = $sandbox2.ProgramData
    UserProfileRoot = $sandbox2.UserProfile
    TempRoot = $sandbox2.Temp
    SystemTempRoot = $sandbox2.SystemTemp
    ManifestPath = $manifestTsv
  }
  $sw.Stop()
  $stream.Close()

  if ($sw.ElapsedMilliseconds -lt 1300) {
    throw ("EXPECT FAILED: locked file must go through bounded retries (3x500ms); elapsed={0}ms" -f $sw.ElapsedMilliseconds)
  }
  if ($lockedRun.ExitCode -eq 0) {
    $scheduled = Get-ScheduledSummary $lockedRun.Output
    if ($null -eq $scheduled -or $scheduled -lt 1) {
      throw 'EXPECT FAILED: locked-file success must report scheduled>=1 in the summary line'
    }
  } else {
    # 非提权：MoveFileEx 登记被拒 → 硬失败是诚实结果；文件必须仍在。
    Assert-PathPresent $lockedFile 'locked file must survive when reboot registration is impossible'
  }
} finally {
  if ($null -ne $stream) { $stream.Close() }
  $env:USERPROFILE = $oldUserProfile
  $env:LOCALAPPDATA = $oldLocalAppData
  $env:ProgramData = $oldProgramData
  if (Test-Path -LiteralPath $sandbox2.Root) {
    Remove-Item -LiteralPath $sandbox2.Root -Recurse -Force
  }
}

Write-Output 'p1_upgrade_data_preserve_contract_test: ok'
