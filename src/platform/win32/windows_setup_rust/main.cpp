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

int RunSilentInstall(const exv::setup::CliOptions &opts) {
  ComInit com;
  exv::setup::InstallRequest req;
  req.install_dir = opts.install_dir.empty() ? exv::setup::DefaultInstallDir() : opts.install_dir;
  // Match NSIS: Start Menu always; desktop/launch from flags (defaults true).
  req.create_desktop_shortcut = opts.desktop_shortcut;
  req.create_start_menu = true;
  req.create_quick_launch = opts.quick_launch;
  req.launch_app = opts.launch_app;
#ifndef EXV_PRODUCT_VERSION
#define EXV_PRODUCT_VERSION L"4.0.0"
#endif
  req.app_version = EXV_PRODUCT_VERSION;
  wchar_t env[4096] = {};
  if (GetEnvironmentVariableW(L"EXV_SETUP_PAYLOAD_DIR", env, 4096) > 0) {
    req.payload_path = env;
    req.payload_is_directory = true;
  }
  const auto result = exv::setup::RunInstall(req, nullptr);
  return result.ok ? 0 : 1;
}

int RunSilentUninstall(const exv::setup::CliOptions &opts) {
  ComInit com;
  exv::setup::UninstallRequest req;
  req.install_dir = opts.install_dir;
  req.clear_user_data = opts.clear_user_data;
  req.user_profile_root = opts.user_profile_root;
  req.local_app_data_root = opts.local_app_data_root;
  req.roaming_app_data_root = opts.roaming_app_data_root;
  req.config_dir = opts.config_dir;
  req.temp_root = opts.temp_root;
  req.include_credential_manager = opts.include_credential_manager;
  req.credential_cleanup_note = opts.credential_cleanup_note;
  const auto result = exv::setup::RunUninstall(req, nullptr);
  return result.ok ? 0 : 1;
}

}  // namespace

int APIENTRY wWinMain(HINSTANCE, HINSTANCE, LPWSTR, int) {
  int argc = 0;
  LPWSTR *argv = CommandLineToArgvW(GetCommandLineW(), &argc);
  if (argv == nullptr) {
    return 2;
  }

  auto parsed = exv::setup::ParseCli(argc, argv);
  LocalFree(argv);

  if (!parsed.has_value()) {
    return 2;
  }

  // 卸载涉及杀进程（StopRunningAppProcesses）、删服务（RunEngineServiceUninstall /
  // StopAndDeleteService）与删文件（DeleteTreeBestEffort，含 MoveFileEx 重启删除登记——
  // 写 HKLM\SYSTEM\...\PendingFileRenameOperations 需要管理员）。清单为 asInvoker，
  // 未提权时所有卸载权限均不足（实测 remove=5/schedule=5 报「无法删除或登记重启删除」）。
  // 对策：卸载角色在派发前自提权——runas 重启自身（携带原卸载参数），原进程退出，
  // 提权实例完成实际卸载。安装角色继续走既有的 ElevatedWorker 提权模式。
  const bool is_uninstall_role = parsed->role == exv::setup::Role::UninstallGui ||
                                 parsed->role == exv::setup::Role::UninstallSilent;
  if (is_uninstall_role) {
    CaptureInitiatingUserContext(*parsed);
  }
  if (is_uninstall_role && !exv::setup::IsProcessElevated()) {
    wchar_t self[MAX_PATH] = {};
    if (GetModuleFileNameW(nullptr, self, MAX_PATH) == 0 || self[0] == 0) {
      return 2;
    }
    const std::wstring params = exv::setup::BuildUninstallElevationParameters(*parsed);
    SHELLEXECUTEINFOW sei = {sizeof(sei)};
    sei.lpVerb = L"runas";
    sei.lpFile = self;
    sei.lpParameters = params.c_str();
    sei.nShow = parsed->role == exv::setup::Role::UninstallSilent ? SW_HIDE : SW_SHOWNORMAL;
    if (!ShellExecuteExW(&sei)) {
      return 2;  // UAC 拒绝/失败 → 不继续非提权卸载（避免再次报权限错）。
    }
    return 0;  // 原进程退出；提权实例完成实际卸载。
  }
  if (is_uninstall_role) {
    ResolveCredentialCleanupScope(*parsed);
  }

  ComInit com;

  switch (parsed->role) {
    case exv::setup::Role::InstallGui:
      return exv::setup::ui::RunInstallerGui(*parsed);
    case exv::setup::Role::UninstallGui:
      return exv::setup::ui::RunUninstallerGui(*parsed);
    case exv::setup::Role::InstallSilent:
      return RunSilentInstall(*parsed);
    case exv::setup::Role::UninstallSilent:
      return RunSilentUninstall(*parsed);
    case exv::setup::Role::ElevatedWorker:
      return exv::setup::RunElevatedWorkerServer(parsed->elevated_pipe, parsed->elevated_token);
  }
  return 2;
}
