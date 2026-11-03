$ErrorActionPreference = 'Stop'

$install = Get-Content -Raw -Encoding UTF8 'install_engine.cpp'
$elevate = Get-Content -Raw -Encoding UTF8 'elevate.cpp'
$service = Get-Content -Raw -Encoding UTF8 'service_control.cpp'
$service_header = Get-Content -Raw -Encoding UTF8 'service_control.hpp'
$ui = Get-Content -Raw -Encoding UTF8 'ui/setup_window.cpp'

function Assert-Contains([string] $text, [string] $pattern, [string] $message) {
  if ($text -notmatch $pattern) {
    throw "EXPECT FAILED: $message"
  }
}

function Assert-NotContains([string] $text, [string] $pattern, [string] $message) {
  if ($text -match $pattern) {
    throw "EXPECT FAILED: $message"
  }
}

# 覆盖安装必须先停止服务，但不能删除原有 SCM 安装记录。
Assert-Contains $service_header 'bool StopService\(' 'service control exposes stop-only operation'
Assert-Contains $service 'bool StopService\(' 'stop-only service operation is implemented'
Assert-Contains $elevate 'StopService\(' 'elevated worker performs stop-only operation'
Assert-Contains $install 'EnsureServiceStopped\(' 'install preflight stops the installed service'
Assert-Contains $install 'return EnsureServiceStopped\(elevation\)' 'install preflight returns stop result on the shared elevation session'

# 解压失败必须把实际原因传到错误页，不能只显示固定占位文案。
Assert-Contains $install 'extraction_error' 'install keeps extraction diagnostic'
Assert-Contains $install 'result\.error \+= L"："' 'install appends extraction diagnostic'

# 错误页控件必须受卡片底部约束，不能再用固定 y 导致按钮溢出。
Assert-Contains $ui 'card\.bottom' 'error page layout uses card bottom boundary'
Assert-NotContains $ui 'ButtonRect\(cx, card\.top \+ 300\.0f\)' 'error close button is not positioned past the card'
Assert-Contains $ui 'card\.top \+ 7[0-9]\.0f' 'error title is moved upward'

# 2026-10-10 裁定：安装程序铺文件前必须撤销 EXV 名下的重启删除登记（上一次
# 卸载可能对安装树内被锁路径登记过 MOVEFILE_DELAY_UNTIL_REBOOT），否则陈旧登记
# 会在安装后的重启时误删新装文件。撤销失败必须 fail-closed 终止安装。
$pfro = Get-Content -Raw -Encoding UTF8 'pfro_sweep.cpp'
$pfroHeader = Get-Content -Raw -Encoding UTF8 'pfro_sweep.hpp'
Assert-Contains $install 'SweepExvPendingFileRenameOperations' 'install preflight sweeps EXV pending reboot deletions before laying files'
Assert-Contains $install 'pfro_sweep\.ok' 'install fails closed when the sweep cannot proceed'
Assert-Contains $pfro 'PendingFileRenameOperations' 'sweep targets the Session Manager PFRO value'
Assert-Contains $pfro '\.exv-uninstall-' 'sweep recognizes the EXV quarantine directory namespace'
Assert-Contains $pfroHeader 'IsExvOwnedPfroSource' 'sweep ownership decision is unit-testable'

# 2026-10-10 修正：安装 GUI 是 asInvoker，写 HKLM 需经 UAC 提权通道——非提权时
# 只读探测，确有 EXV 登记才弹一次 UAC 走 worker 执行撤销；worker 失败仍 fail-closed。
$elevate = Get-Content -Raw -Encoding UTF8 'elevate.cpp'
Assert-Contains $install 'needs_elevation' 'install routes write-less sweep results through elevation'
Assert-Contains $install 'RunElevatedPfroSweep' 'install invokes the elevated sweep worker when needed'
Assert-Contains $elevate 'sweep_pfro' 'elevated worker speaks the sweep_pfro operation'
Assert-Contains $elevate 'RunElevatedPfroSweep' 'elevation exposes the PFRO sweep entry point'

# S2（2026-10-10 单次 UAC 提权会话，计划 §五 v2）：停服/清扫/起服共用一个
# runas worker 会话——首个提权步骤前一次 UAC、会话内多 OP 复用同一进程、
# 600s 空闲上限 + 30s 心跳覆盖最坏安装时长；PFRO 的第二套独立实现（-pfro
# 管道）移除并入同一会话；会话中途死亡由下一提权步骤按需重建（重建=再弹
# 一次 UAC，经注入缝 g_elevated_launch 计数，真机单 UAC 人工验收归 W5）。
Assert-Contains $install 'CreateElevationSession' 'install runs all elevated steps through one shared session'
Assert-Contains $install 'RunElevatedPfroSweep\(\*elevation' 'PFRO sweep is issued on the shared elevation session'
Assert-Contains $elevate 'class RunasElevatedSession' 'client session object drives one runas worker per install'
Assert-Contains $elevate 'g_elevated_launch' 'runas launches go through the injection seam (UAC count observable)'
Assert-Contains $elevate 'kSessionIdleLimitMs = 600000' 'session idle limit is raised to 600s'
Assert-Contains $elevate 'kHeartbeatIntervalMs = 30000' '30s heartbeat covers the worst-case install duration'
Assert-Contains $elevate '"OP ping' 'heartbeat keeps the session worker alive between commands'
Assert-NotContains $elevate 'L"-pfro"' 'the separate PFRO pipe implementation is gone (merged session)'
Assert-NotContains $elevate 'bool RunElevatedWorker\(' 'the per-operation runas client entry point is replaced by the session'

# S5（2026-10-10 换盘服务迁移，计划 §六 v2）：覆盖安装不再只停不改 ImagePath——
# 停服前 QueryServiceConfigW 读 ImagePath 判漂移（引号/大小写/斜杠规整比对），
# 漂移经同一提权会话的 migrate_service OP 改写（ChangeServiceConfigW 需
# SERVICE_CHANGE_CONFIG，asInvoker GUI 拿不到）；尾参（--service --control-pipe
# <sid 派生管道名> --dll --user-sid --config-dir）与安装目录无关，必须原样保留；
# 变更失败 fail-closed 拒装且文案含新旧路径；写后回读校验。
Assert-Contains $service_header 'QueryServiceImagePath' 'service control exposes the image path query'
Assert-Contains $service_header 'MigrateServiceImagePath' 'service control exposes the image path migration'
Assert-Contains $service 'QueryServiceConfigW' 'image path is read through QueryServiceConfigW'
Assert-Contains $service 'ChangeServiceConfigW' 'image path is rewritten through ChangeServiceConfigW'
Assert-Contains $service 'NormalizeServiceImagePathForCompare' 'image path comparison normalizes quotes/case/slashes'
Assert-Contains $service 'ServiceImagePathDrifted' 'image path drift decision composes split and form normalization'
Assert-Contains $service 'SERVICE_CHANGE_CONFIG' 'migration opens the service with SERVICE_CHANGE_CONFIG'
Assert-Contains $service 'parts\.arguments' 'migration preserves the launch-argument tail verbatim'
Assert-Contains $install 'QueryServiceImagePath\(\)' 'install preflight reads the service image path before stopping'
Assert-Contains $install 'ServiceImagePathDrifted' 'install decides drift on the normalized image path'
Assert-Contains $install 'EnsureServiceImagePathMigrated\(\*elevation' 'image path migration is issued on the shared elevation session'
Assert-Contains $install 'needs_service_migration' 'install gates the migration on detected drift'
Assert-Contains $install '旧：' 'migration failure text names the old image path'
Assert-Contains $install '新：' 'migration failure text names the new image path'
Assert-Contains $elevate 'migrate_service' 'elevated worker speaks the migrate_service operation'
Assert-Contains $elevate 'WideCharToMultiByte\(CP_UTF8' 'migrate path travels the narrow pipe losslessly via UTF-8'

# S3b（2026-10-10 安装侧清单消费，计划 §七 v2）：解压完成后立即做安装目录
# diff——install_manifest.tsv 的 installdir 行是判册，磁盘多余文件三族判定：
# payload/legacy-payload（含历代退役行）→删，runtime-generated→跳过，
# 未知→报告不删（防清单库自身漏项）；清单缺失 fail-closed 拒装（对齐 S3c）；
# 清理明细+未知清单记入 InstallResult.cleanup 并在完成页/日志如实呈现。
$installHeader = Get-Content -Raw -Encoding UTF8 'install_engine.hpp'
Assert-Contains $install 'ParseInstallManifestForDiff' 'install parses the manifest library for the dir diff'
Assert-Contains $install 'DecideInstallDirDiff' 'install diff decision is a named pure function (unit-testable)'
Assert-Contains $install 'install_manifest\.tsv' 'install locates the manifest via the -ManifestPath convention candidates'
Assert-Contains $install '安装清单缺失' 'missing manifest fails the install closed (S3c-aligned)'
Assert-Contains $install '不在册文件' 'unknown files are reported, not deleted'
Assert-Contains $installHeader 'InstallCleanupDetail' 'InstallResult carries the S3b cleanup detail fields'
Assert-Contains $installHeader 'manifest_path' 'InstallRequest carries the explicit manifest path parameter'

# S6（2026-10-10 错误页重试，计划 §七 v2）：安装失败页一律显示「重试」——安装
# 同目录幂等覆盖重试安全；卸载错误页不加（显式注释声明：卸载链重跑幂等性
# 差）；重试=完整重置 StartInstallWorker 守卫态（work_running/
# deferred_work_started/intro_drain_active/水面进度）后重跑 RunInstall，
# install_dir 复用本次请求值。
Assert-Contains $ui 'hit_retry' 'error page exposes the retry hit target for install failures'
Assert-Contains $ui 'L"重试"' 'error page shows the retry button label'
Assert-Contains $ui 'RetryInstallFromErrorPage' 'retry goes through the guard-state reset entry point'
Assert-Contains $ui '卸载错误页不加' 'the uninstall error page opt-out is declared in a comment'
Assert-Contains $ui 'install_cleanup_detail' 'finish page renders the S3b cleanup detail'

# T8（F12）版本同源：GUI 安装路径的 app_version 必须来自 version_const.hpp 的
# EXV_PRODUCT_VERSION（最终写入 ARP DisplayVersion），禁止硬编码版本串或本地
# #define 双定义。S1（2026-10-10 静默移除）：main.cpp 的静默安装路径已删除，
# 版本同源断言收敛到 GUI 路径，main.cpp 不得残留静默入口或本地宏定义。
$main = Get-Content -Raw -Encoding UTF8 'main.cpp'
$versionHeader = Get-Content -Raw -Encoding UTF8 'version_const.hpp'
Assert-Contains $ui 'app_version = EXV_PRODUCT_VERSION' 'GUI install sources app_version from the shared version constant'
Assert-NotContains $ui '"3\.3\.8"' 'GUI install no longer hardcodes a stale product version'
Assert-Contains $ui 'version_const\.hpp' 'GUI install includes the shared version constant header'
Assert-NotContains $main '#define EXV_PRODUCT_VERSION' 'main no longer redefines the version macro locally'
Assert-NotContains $main 'RunSilentInstall' 'silent install entry point removed (S1)'
Assert-NotContains $main 'RunSilentUninstall' 'silent uninstall entry point removed (S1)'
Assert-Contains $versionHeader '#ifndef EXV_PRODUCT_VERSION' 'version header keeps a guarded fallback default'

# S1（2026-10-10 静默移除，R-QUIET 关闭）：QuietUninstallString 不再写入——静默
# 卸载已移除，白名单外参数（含 /S）以退出码 3 拒绝；安装时必须清理旧版本写入
# 的存量值（RegDeleteValueExW），避免 ARP 残留指向失效的 /S 调用形态。
$registry = Get-Content -Raw -Encoding UTF8 'registry_install.cpp'
Assert-NotContains $registry 'WriteString\(un, L"QuietUninstallString"' 'uninstall registry no longer writes the quiet uninstall string (S1)'
Assert-Contains $registry 'RegDeleteValueW\(un, L"QuietUninstallString"' 'install clears the legacy quiet uninstall string value'

# T7（F6）服务回滚：original_service 为 Running 且已执行停服后，任一失败 return
# 前 best-effort EnsureServiceRunning 恢复，恢复结果并入错误文案（含二次 UAC
# 提示——非提权 GUI 下恢复走 runas）。
Assert-Contains $install 'attempt_service_recovery' 'install wires best-effort service recovery after the pre-stop'
Assert-Contains $install '已尝试恢复服务运行' 'install folds the service recovery outcome into failure text'
Assert-Contains $install '恢复可能再次请求管理员确认' 'install warns that recovery may re-prompt for elevation'
$recoveryCallSites = ([regex]::Matches($install, 'attempt_service_recovery\(\);')).Count
if ($recoveryCallSites -lt 10) {
  throw "EXPECT FAILED: install routes every post-stop failure return through service recovery (found $recoveryCallSites call sites, need >= 10)"
}

# T7（F7）解压重试：输出文件打开/写失败按 uninstaller_copy.cpp 同参数有界重试
# （5 秒窗口/100 毫秒间隔，锁类错误判定），重试耗尽才整包失败。
$archive = Get-Content -Raw -Encoding UTF8 'payload/archive_reader.cpp'
Assert-Contains $archive 'kRetryWindowMs = 5000' 'archive extraction uses the shared 5s retry window'
Assert-Contains $archive 'kRetryIntervalMs = 100' 'archive extraction uses the shared 100ms retry interval'
Assert-Contains $archive 'IsRetryableLockError' 'archive extraction retries only lock-class errors'

Write-Output 'p1_overwrite_install_contract_test: ok'
