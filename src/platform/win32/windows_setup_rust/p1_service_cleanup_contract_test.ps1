$ErrorActionPreference = 'Stop'

$service = Get-Content -Raw 'service_control.cpp'
$service_header = Get-Content -Raw 'service_control.hpp'
$uninstall = Get-Content -Raw 'uninstall_engine.cpp'
$process = Get-Content -Raw 'process_control.cpp'
$process_header = Get-Content -Raw 'process_control.hpp'

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

Assert-Contains $service_header 'enum class ServicePresence' 'SCM lookup exposes an error state'
Assert-Contains $service 'ERROR_SERVICE_DOES_NOT_EXIST' 'missing service is classified as idempotent absence'
Assert-NotContains $service 'return !IsServiceInstalled\(service_name\)' 'SCM open failure is not treated as service absence'
Assert-Contains $service 'ControlService\(' 'stop result is examined by service removal'
Assert-Contains $service 'QueryServiceStatus\(' 'service stop polling remains observable'
Assert-Contains $service 'DeleteService\(' 'delete result is examined by service removal'
Assert-Contains $service 'ERROR_SERVICE_MARKED_FOR_DELETE' 'marked-for-delete is only accepted with verification'
Assert-Contains $service 'QueryServicePresence' 'wait path distinguishes absent from SCM query failure'

Assert-Contains $uninstall 'ServicePresence::Error' 'uninstall reports SCM query failure'
Assert-Contains $uninstall 'StopAndDeleteService' 'uninstall consumes service removal failure'
Assert-Contains $uninstall 'DeleteTreeResult' 'file cleanup returns an aggregate result'
Assert-Contains $uninstall 'MoveFileExW' 'delayed deletion is accepted only through an explicit API result'
Assert-Contains $uninstall 'result.error' 'uninstall exposes cleanup failures'
Assert-Contains $uninstall 'if \(!errors\.empty\(\)\)' 'uninstall gates success on the aggregate error list'

# 2026-10-10 修复计划 T3：死管道负向断言——C++ 时代的 exv-core-ipc-v1 优雅
# 停机是恒不可达死代码，必须保持已删除（F3），不得回潮。
Assert-NotContains $process 'exv-core-ipc-v1' 'legacy core IPC pipe stays deleted from process control'
Assert-NotContains $process 'TryGracefulCoreShutdown' 'dead graceful shutdown path stays deleted'
Assert-NotContains $process 'core\.shutdown' 'legacy core.shutdown JSON line stays deleted'
Assert-NotContains $process_header 'TryGracefulCoreShutdown' 'dead graceful shutdown declaration stays deleted'

# 2026-10-10 修复计划 T3：杀序断言——exv-ui → exv-core → exv-engine 形态分叉。
$kill_ui = $process.IndexOf('TerminateImageInstances(kUiImageName')
$kill_core = $process.IndexOf('TerminateImageInstances(kCoreImageName')
if ($kill_ui -lt 0 -or $kill_core -lt 0 -or $kill_ui -gt $kill_core) {
  throw 'EXPECT FAILED: kill order must terminate exv-ui before exv-core'
}
Assert-Contains $process 'QueryServicePresence\(kEngineServiceName\)' 'engine disposition forks on SCM service presence'
Assert-Contains $process 'StopOneshotEngineInstances' 'oneshot engine form waits for self-exit'
Assert-Contains $process 'kEngineSelfExitWaitMs = 15000' 'engine self-exit wait budget is at least 15s (5s assembly cancel + 8s NRPT cleanup + 300ms flush + margin)'
Assert-Contains $process 'TaskKillImage\(kUiImageName\)' 'taskkill fallback keeps covering ui/core residuals'
Assert-Contains $process 'TaskKillImage\(kCoreImageName\)' 'taskkill fallback keeps covering ui/core residuals'

# 卸载时序：服务移除块保持在 StopRunningAppProcesses 之后（T3 步骤 3：形态
# 分叉已保证 engine 优雅清理在杀进程阶段完成，服务移除无须整体前移）。
$stop_processes = $uninstall.IndexOf('StopRunningAppProcesses()')
$remove_service = $uninstall.IndexOf('QueryServicePresence(kEngineServiceName)')
if ($stop_processes -lt 0 -or $remove_service -lt 0 -or $stop_processes -gt $remove_service) {
  throw 'EXPECT FAILED: uninstall must stop running app processes before service removal'
}

# 2026-10-10 修复计划 T10（F13，v2 定案）：over-the-shoulder 提权时卸载侧身份
# 错位治理。检测复用 credential 路径先例（main.cpp ResolveCredentialCleanupScope
# 的 SID 比对结果经 UninstallRequest 携带）；快捷方式清理按发起用户根参数化；
# 安装侧创建路径维持 SHGetKnownFolderPath 现状。本文件无 BOM，断言锚点一律用
# ASCII 标识符（中文文案断言在 tests/uninstall_result_test.cpp 内做）。
$shortcuts = Get-Content -Raw 'shortcuts.cpp'
$shortcuts_header = Get-Content -Raw 'shortcuts.hpp'
Assert-Contains $uninstall 'ElevatedIdentityDiffersFromInitiator' 'uninstall reuses the credential-scope SID comparison verdict for identity mismatch'
Assert-Contains $uninstall 'BuildElevatedIdentityResidualNotice' 'identity mismatch surfaces an honest HKCU residual notice'
Assert-Contains $uninstall 'kUrlSchemeKey' 'residual notice names the aTrust user-level protocol key'
Assert-Contains $uninstall 'kUninstallKey' 'residual notice names the per-user uninstall registry key'
Assert-Contains $uninstall 'RemoveDesktopShortcut\(identity_mismatch \? request\.user_profile_root' 'desktop shortcut cleanup is parameterized on the initiating user root'
Assert-Contains $uninstall 'RemoveStartMenuShortcuts\(identity_mismatch \? request\.roaming_app_data_root' 'start-menu shortcut cleanup is parameterized on the initiating roaming root'
Assert-Contains $uninstall 'RemoveQuickLaunchShortcut\(identity_mismatch \? request\.roaming_app_data_root' 'quick-launch shortcut cleanup is parameterized on the initiating roaming root'
Assert-Contains $shortcuts_header 'RemoveDesktopShortcut\(const std::wstring &user_profile_root = \{\}\)' 'desktop removal accepts the initiating profile root with known-folder fallback'
Assert-Contains $shortcuts_header 'RemoveStartMenuShortcuts\(const std::wstring &roaming_app_data_root = \{\}\)' 'start-menu removal accepts the initiating roaming root with known-folder fallback'
Assert-Contains $shortcuts_header 'RemoveQuickLaunchShortcut\(const std::wstring &roaming_app_data_root = \{\}\)' 'quick-launch removal accepts the initiating roaming root with known-folder fallback'
Assert-Contains $shortcuts 'FOLDERID_Desktop' 'desktop removal keeps the known-folder fallback'
Assert-Contains $shortcuts 'FOLDERID_Programs' 'start-menu removal keeps the known-folder fallback'
Assert-Contains $shortcuts 'FOLDERID_RoamingAppData' 'quick-launch removal keeps the known-folder fallback'
Assert-Contains $shortcuts 'L"Desktop"' 'desktop derivation follows the standard profile layout'
Assert-Contains $shortcuts 'L"Start Menu"' 'start-menu derivation follows the standard roaming layout'
Assert-Contains $shortcuts 'L"Internet Explorer"' 'quick-launch derivation follows the legacy roaming layout'
Assert-NotContains $shortcuts_header 'CreateDesktopShortcut\(const std::wstring &user_profile_root' 'install-side shortcut creation stays unparameterized (v2: install path unchanged)'

# S2（2026-10-10 单次 UAC 提权会话，计划 §五 v2）：客户端会话对象取代
# 「一命令一进程」与 PFRO 第二套独立实现——两者并入同一 runas worker 会话
#（本文件无 BOM，锚点一律 ASCII 标识符）。停服/清扫/起服共用一个 worker，
# 空闲上限 600s + 30s 心跳；会话死亡由下一提权步骤按需重建。
$elevate = Get-Content -Raw 'elevate.cpp'
$elevate_header = Get-Content -Raw 'elevate.hpp'
Assert-Contains $elevate_header 'class ElevatedSession' 'client elevation session interface exists'
Assert-Contains $elevate_header 'CreateElevationSession' 'session factory is declared for install-side callers'
Assert-Contains $elevate 'CreateElevationSession' 'production session implementation exists'
Assert-Contains $elevate 'kSessionIdleLimitMs = 600000' 'worker idle limit is the session-scale 600s budget'
Assert-NotContains $elevate 'L"-pfro"' 'the standalone PFRO pipe suffix is removed (merged into the session)'
Assert-NotContains $elevate 'bool RunElevatedWorker\(' 'the per-operation runas client entry point is gone'

Write-Output 'p1_service_cleanup_contract_test: ok'
