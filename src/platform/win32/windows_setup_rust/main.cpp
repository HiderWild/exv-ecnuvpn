#include "windows_setup_rust/cli.hpp"
#include "windows_setup_rust/elevate.hpp"
#include "windows_setup_rust/install_engine.hpp"
#include "windows_setup_rust/ui/setup_window.hpp"
#include "windows_setup_rust/uninstall_engine.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shellapi.h>
#include <objbase.h>

#include <cwchar>
#include <vector>

namespace {

class ComInit {
 public:
  ComInit() {
    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  }
  ~ComInit() {
    CoUninitialize();
  }
};

std::wstring ReadEnvironmentVariable(const wchar_t *name) {
  const DWORD required = GetEnvironmentVariableW(name, nullptr, 0);
  if (required == 0) {
    return {};
  }
  std::vector<wchar_t> value(required);
  const DWORD copied =
      GetEnvironmentVariableW(name, value.data(), static_cast<DWORD>(value.size()));
  if (copied == 0 || copied >= value.size()) {
    return {};
  }
  return std::wstring(value.data(), copied);
}

std::wstring ReadCurrentTempRoot() {
  wchar_t path[MAX_PATH + 1] = {};
  const DWORD length = GetTempPathW(MAX_PATH + 1, path);
  if (length == 0 || length > MAX_PATH) {
    return {};
  }
  return std::wstring(path, length);
}

void CaptureInitiatingUserContext(exv::setup::CliOptions &options) {
  if (options.initiating_user_context) {
    return;
  }
  options.user_profile_root = ReadEnvironmentVariable(L"USERPROFILE");
  options.local_app_data_root = ReadEnvironmentVariable(L"LOCALAPPDATA");
  options.roaming_app_data_root = ReadEnvironmentVariable(L"APPDATA");
  options.config_dir = ReadEnvironmentVariable(L"EXV_CONFIG_DIR");
  options.temp_root = ReadCurrentTempRoot();
  options.initiating_user_sid = exv::setup::CurrentProcessUserSid();
  // Preserve the fact that capture already happened even if an environment value or
  // SID was unavailable. The elevated child must not backfill it from another user.
  options.initiating_user_context = true;
}

void ResolveCredentialCleanupScope(exv::setup::CliOptions &options) {
  const auto elevated_sid = exv::setup::CurrentProcessUserSid();
  options.include_credential_manager =
      !options.initiating_user_sid.empty() && !elevated_sid.empty() &&
      _wcsicmp(options.initiating_user_sid.c_str(), elevated_sid.c_str()) == 0;
  if (options.include_credential_manager) {
    options.credential_cleanup_note.clear();
  } else if (!options.initiating_user_sid.empty() && !elevated_sid.empty()) {
    options.credential_cleanup_note =
        L"提权身份与发起卸载者不同，未访问原用户的凭据管理器。";
  } else {
    options.credential_cleanup_note =
        L"无法确认发起卸载者与当前提权身份一致，未访问原用户的凭据管理器。";
  }
}

// S1（2026-10-10 静默移除）：白名单外参数（含 /S）以退出码 3 拒绝，无任何 UI；
// CLI 发起时向父控制台回一行文字提示。已知边界（复审 MINOR-5，如实记录）：
//   - 本进程为 WIN32 GUI 子系统，父进程无控制台（GUI 双击/资源管理器启动）时
//     AttachConsole 失败，静默跳过——退出码 3 仍然生效；
//   - 父进程重定向 stdout（如 PowerShell `$x = & setup /bad`）时文字写入
//     CONOUT$ 而非管道，重定向读端收不到该行——不做额外补偿。
void ReportInvalidArgumentToParentConsole(const std::wstring &arg) {
  if (!AttachConsole(ATTACH_PARENT_PROCESS)) {
    return;  // 无父控制台：静默跳过（仅退出码 3 生效）。
  }
  const std::wstring message =
      L"参数非法：" + arg + L"（用法：无参数启动图形安装器 / /D=<目录>）\r\n";
  HANDLE console = CreateFileW(L"CONOUT$", GENERIC_WRITE, FILE_SHARE_WRITE, nullptr,
                               OPEN_EXISTING, 0, nullptr);
  if (console != INVALID_HANDLE_VALUE) {
    DWORD written = 0;
    WriteConsoleW(console, message.c_str(), static_cast<DWORD>(message.size()), &written,
                  nullptr);
    CloseHandle(console);
  }
  FreeConsole();
}

}  // namespace

int APIENTRY wWinMain(HINSTANCE, HINSTANCE, LPWSTR, int) {
  int argc = 0;
  LPWSTR *argv = CommandLineToArgvW(GetCommandLineW(), &argc);
  if (argv == nullptr) {
    return 2;
  }

  const exv::setup::CliParseResult parsed_result = exv::setup::ParseCli(argc, argv);
  LocalFree(argv);

  // S1：白名单外参数（含 /S）→ 退出码 3（参数非法），无任何 UI；CLI 文字提示
  // 见 ReportInvalidArgumentToParentConsole。不可恢复解析错误维持退出码 2。
  if (parsed_result.status == exv::setup::CliParseStatus::InvalidArgument) {
    ReportInvalidArgumentToParentConsole(parsed_result.rejected_argument);
    return 3;
  }
  if (parsed_result.status != exv::setup::CliParseStatus::Ok) {
    return 2;
  }
  // 可变副本：卸载角色的发起者上下文捕获/凭据域解析需要在派发前就地补全。
  exv::setup::CliOptions parsed = parsed_result.options;

  // 卸载涉及杀进程（StopRunningAppProcesses）、删服务（RunEngineServiceUninstall /
  // StopAndDeleteService）与删文件（DeleteTreeBestEffort，含 MoveFileEx 重启删除登记——
  // 写 HKLM\SYSTEM\...\PendingFileRenameOperations 需要管理员）。清单为 asInvoker，
  // 未提权时所有卸载权限均不足（实测 remove=5/schedule=5 报「无法删除或登记重启删除」）。
  // 对策：卸载角色在派发前自提权——runas 重启自身（携带原卸载参数），原进程退出，
  // 提权实例完成实际卸载。安装角色继续走既有的 ElevatedWorker 提权模式。
  const bool is_uninstall_role = parsed.role == exv::setup::Role::UninstallGui;
  if (is_uninstall_role) {
    CaptureInitiatingUserContext(parsed);
  }
  if (is_uninstall_role && !exv::setup::IsProcessElevated()) {
    wchar_t self[MAX_PATH] = {};
    if (GetModuleFileNameW(nullptr, self, MAX_PATH) == 0 || self[0] == 0) {
      return 2;
    }
    const std::wstring params = exv::setup::BuildUninstallElevationParameters(parsed);
    SHELLEXECUTEINFOW sei = {sizeof(sei)};
    sei.lpVerb = L"runas";
    sei.lpFile = self;
    sei.lpParameters = params.c_str();
    sei.nShow = SW_SHOWNORMAL;  // S1：静默角色已移除，往返一律可见窗口。
    if (!ShellExecuteExW(&sei)) {
      return 2;  // UAC 拒绝/失败 → 不继续非提权卸载（避免再次报权限错）。
    }
    return 0;  // 原进程退出；提权实例完成实际卸载。
  }
  if (is_uninstall_role) {
    ResolveCredentialCleanupScope(parsed);
  }

  ComInit com;

  switch (parsed.role) {
    case exv::setup::Role::InstallGui:
      return exv::setup::ui::RunInstallerGui(parsed);
    case exv::setup::Role::UninstallGui:
      return exv::setup::ui::RunUninstallerGui(parsed);
    case exv::setup::Role::ElevatedWorker:
      return exv::setup::RunElevatedWorkerServer(parsed.elevated_pipe, parsed.elevated_token,
                                                 parsed.elevated_pfro_sweep_dir);
  }
  return 2;
}
