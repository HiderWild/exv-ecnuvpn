// 调用真实 RunUninstall；仅在 Win32/宿主副作用边界注入确定性 fake。
// 文件均落在当前测试构建目录的独有子目录；不会操作真实安装、服务或用户配置。
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

namespace fixture {
BOOL WINAPI ScheduleDelete(LPCWSTR existing, LPCWSTR replacement, DWORD flags);
}

#define MoveFileExW fixture::ScheduleDelete
#include "windows_setup_rust/uninstall_engine.cpp"
#undef MoveFileExW

namespace fixture {
exv::setup::ServicePresence service_presence = exv::setup::ServicePresence::NotInstalled;
exv::setup::HiddenProcessResult cleanup_result;
std::vector<std::wstring> cleanup_commands;
std::vector<std::wstring> scheduled_paths;

void Reset() {
  service_presence = exv::setup::ServicePresence::NotInstalled;
  cleanup_result = {};
  cleanup_commands.clear();
  scheduled_paths.clear();
}

BOOL WINAPI ScheduleDelete(LPCWSTR existing, LPCWSTR replacement, DWORD flags) {
  if (!existing || replacement != nullptr || flags != MOVEFILE_DELAY_UNTIL_REBOOT) {
    SetLastError(ERROR_INVALID_PARAMETER);
    return FALSE;
  }
  scheduled_paths.emplace_back(existing);
  return TRUE;
}
}  // namespace fixture

namespace exv::setup {
std::wstring ReadRegisteredInstallDir() { return {}; }
void StopRunningAppProcesses() {}
ServicePresence QueryServicePresence(const std::wstring &) { return fixture::service_presence; }
bool RunEngineServiceUninstall(const std::wstring &) { return true; }
bool WaitForServiceRemoved(const std::wstring &, int, int) { return true; }
bool IsServiceInstalled(const std::wstring &) { return false; }
bool StopAndDeleteService(const std::wstring &) { return true; }
bool RemoveDesktopShortcut() { return true; }
bool RemoveStartMenuShortcuts() { return true; }
bool RemoveQuickLaunchShortcut() { return true; }
bool RemoveInstallRegistry() { return true; }
HiddenProcessResult RunHidden(const std::wstring &, const std::wstring &command_line, int) {
  fixture::cleanup_commands.push_back(command_line);
  return fixture::cleanup_result;
}
}  // namespace exv::setup

#define CHECK(expression)                                                                  \
  do {                                                                                     \
    if (!(expression)) {                                                                   \
      std::cerr << "FAIL line " << __LINE__ << ": " #expression << '\n';                  \
      return 1;                                                                            \
    }                                                                                      \
  } while (false)

namespace {
namespace fs = std::filesystem;

struct CleanupTree {
  fs::path path;
  ~CleanupTree() {
    std::error_code ec;
    fs::remove_all(path, ec);
  }
};

fs::path MakeInstallDir(const fs::path &root, const wchar_t *name) {
  const auto install_dir = root / name;
  fs::create_directories(install_dir);
  std::ofstream(install_dir / "locked.bin", std::ios::binary) << "locked";
  return install_dir;
}

HANDLE LockWithoutDeleteSharing(const fs::path &path) {
  return CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING,
                     FILE_ATTRIBUTE_NORMAL, nullptr);
}

std::string ReadAll(const fs::path &path) {
  std::ifstream input(path, std::ios::binary);
  std::ostringstream output;
  output << input.rdbuf();
  return output.str();
}
}  // namespace

int main() {
  using namespace exv::setup;

  const auto root = fs::current_path() /
                    (L"uninstall-result-test-" + std::to_wstring(GetCurrentProcessId()));
  CHECK(!fs::exists(root));
  fs::create_directory(root);
  CleanupTree cleanup{root};

  // 成功登记重启删除时，最终结果必须保留事实与准确的登记路径数量。
  fixture::Reset();
  const auto delayed_dir = MakeInstallDir(root, L"delayed success");
  HANDLE delayed_lock = LockWithoutDeleteSharing(delayed_dir / "locked.bin");
  CHECK(delayed_lock != INVALID_HANDLE_VALUE);
  UninstallRequest delayed_request;
  delayed_request.install_dir = delayed_dir.wstring();
  const auto delayed_result = RunUninstall(delayed_request);
  CloseHandle(delayed_lock);
  CHECK(delayed_result.ok);
  CHECK(delayed_result.reboot_required);
  CHECK(delayed_result.scheduled_delete_count == 2);
  CHECK(delayed_result.error.empty());
  const auto delayed_detail = BuildUninstallResultDetail(delayed_result);
  CHECK(delayed_detail.find(L"部分文件将在重启后自动删除") != std::wstring::npos);

  // 其他步骤失败不能覆盖已成功登记的重启删除事实；错误页正文必须同时展示两者。
  fixture::Reset();
  fixture::service_presence = ServicePresence::Error;
  const auto partial_dir = MakeInstallDir(root, L"partial failure");
  HANDLE partial_lock = LockWithoutDeleteSharing(partial_dir / "locked.bin");
  CHECK(partial_lock != INVALID_HANDLE_VALUE);
  UninstallRequest partial_request;
  partial_request.install_dir = partial_dir.wstring();
  const auto partial_result = RunUninstall(partial_request);
  CloseHandle(partial_lock);
  CHECK(!partial_result.ok);
  CHECK(partial_result.reboot_required);
  CHECK(partial_result.scheduled_delete_count == 2);
  const auto partial_detail = BuildUninstallResultDetail(partial_result);
  CHECK(partial_detail.find(L"无法查询 exv-engine 服务状态") != std::wstring::npos);
  CHECK(partial_detail.find(L"部分文件将在重启后自动删除") != std::wstring::npos);
  CHECK(partial_detail.rfind(L"部分文件将在重启后自动删除", 0) == 0);

  // 清理脚本收到准确的自定义安装目录；失败输出只写安全临时日志，UI 正文展示路径。
  fixture::Reset();
  fixture::cleanup_result.started = true;
  fixture::cleanup_result.exit_code = 73;
  fixture::cleanup_result.captured_stdout =
      "profile-A cleanup failed\r\ncredential-B cleanup failed\r\n";
  const auto script_dir = root / L"custom install";
  fs::create_directories(script_dir / L"support");
  std::ofstream(script_dir / L"support" / L"clear-local-user-config.ps1") << "# fixture";
  UninstallRequest script_request;
  script_request.install_dir = script_dir.wstring();
  script_request.clear_user_data = true;
  script_request.user_profile_root = (root / L"initiating profile").wstring();
  script_request.local_app_data_root = (root / L"initiating local").wstring();
  script_request.roaming_app_data_root = (root / L"initiating roaming").wstring();
  script_request.config_dir = (root / L"initiating config").wstring();
  script_request.temp_root = (root / L"initiating temp").wstring();
  fs::create_directories(script_request.temp_root);
  const auto script_result = RunUninstall(script_request);
  CHECK(!script_result.ok);
  CHECK(fixture::cleanup_commands.size() == 1);
  const auto &command = fixture::cleanup_commands.front();
  CHECK(command.find(L"-Force -IncludeCredentialManager") != std::wstring::npos);
  CHECK(command.find(L"-InstallDir \"" + script_dir.wstring() + L"\"") != std::wstring::npos);
  CHECK(command.find(L"-UserProfileRoot \"" + script_request.user_profile_root + L"\"") !=
        std::wstring::npos);
  CHECK(command.find(L"-LocalAppDataRoot \"" + script_request.local_app_data_root + L"\"") !=
        std::wstring::npos);
  CHECK(command.find(L"-RoamingAppDataRoot \"" + script_request.roaming_app_data_root + L"\"") !=
        std::wstring::npos);
  CHECK(command.find(L"-ConfigDir \"" + script_request.config_dir + L"\"") !=
        std::wstring::npos);
  CHECK(command.find(L"-TempRoot \"" + script_request.temp_root + L"\"") !=
        std::wstring::npos);
  CHECK(!script_result.diagnostic_log_path.empty());
  CHECK(fs::exists(script_result.diagnostic_log_path));
  const auto diagnostic = ReadAll(script_result.diagnostic_log_path);
  CHECK(diagnostic.find("exit_code=73") != std::string::npos);
  CHECK(diagnostic.find(fixture::cleanup_result.captured_stdout) != std::string::npos);
  const auto script_detail = BuildUninstallResultDetail(script_result);
  CHECK(script_detail.find(script_result.diagnostic_log_path) != std::wstring::npos);
  CHECK(script_detail.find(L"profile-A cleanup failed") == std::wstring::npos);
  std::error_code log_remove_error;
  fs::remove(script_result.diagnostic_log_path, log_remove_error);
  CHECK(!log_remove_error);

  // 另一管理员完成 UAC 时不可清理该管理员的凭据；用户数据路径仍来自原发起者。
  fixture::Reset();
  fixture::cleanup_result.started = true;
  fixture::cleanup_result.exit_code = 0;
  const auto different_admin_dir = root / L"different admin";
  fs::create_directories(different_admin_dir / L"support");
  std::ofstream(different_admin_dir / L"support" / L"clear-local-user-config.ps1") << "# fixture";
  UninstallRequest different_admin_request;
  different_admin_request.install_dir = different_admin_dir.wstring();
  different_admin_request.clear_user_data = true;
  different_admin_request.user_profile_root = (root / L"original user").wstring();
  different_admin_request.local_app_data_root = (root / L"original local").wstring();
  different_admin_request.roaming_app_data_root = (root / L"original roaming").wstring();
  different_admin_request.temp_root = (root / L"original temp").wstring();
  fs::create_directories(different_admin_request.temp_root);
  different_admin_request.include_credential_manager = false;
  different_admin_request.credential_cleanup_note =
      L"提权身份与发起卸载者不同，未访问原用户的凭据管理器。";
  const auto different_admin_result = RunUninstall(different_admin_request);
  CHECK(different_admin_result.ok);
  CHECK(fixture::cleanup_commands.size() == 1);
  CHECK(fixture::cleanup_commands.front().find(L"-IncludeCredentialManager") ==
        std::wstring::npos);
  CHECK(fixture::cleanup_commands.front().find(
            L"-UserProfileRoot \"" + different_admin_request.user_profile_root + L"\"") !=
        std::wstring::npos);
  CHECK(BuildUninstallResultDetail(different_admin_result)
            .find(L"未访问原用户的凭据管理器") != std::wstring::npos);

  // 缺失原用户路径时必须停止用户数据脚本，不能回退到提权管理员环境。
  fixture::Reset();
  const auto missing_scope_dir = root / L"missing original scope";
  fs::create_directories(missing_scope_dir / L"support");
  std::ofstream(missing_scope_dir / L"support" / L"clear-local-user-config.ps1") << "# fixture";
  UninstallRequest missing_scope_request;
  missing_scope_request.install_dir = missing_scope_dir.wstring();
  missing_scope_request.clear_user_data = true;
  const auto missing_scope_result = RunUninstall(missing_scope_request);
  CHECK(!missing_scope_result.ok);
  CHECK(fixture::cleanup_commands.empty());
  CHECK(BuildUninstallResultDetail(missing_scope_result)
            .find(L"未捕获发起卸载者路径") != std::wstring::npos);

  // “保留用户数据”不调用脚本，即使安装目录中存在该脚本。
  fixture::Reset();
  const auto preserved_dir = root / L"preserve data";
  fs::create_directories(preserved_dir / L"support");
  std::ofstream(preserved_dir / L"support" / L"clear-local-user-config.ps1") << "# fixture";
  UninstallRequest preserved_request;
  preserved_request.install_dir = preserved_dir.wstring();
  preserved_request.clear_user_data = false;
  const auto preserved_result = RunUninstall(preserved_request);
  CHECK(preserved_result.ok);
  CHECK(fixture::cleanup_commands.empty());

  std::cout << "PASS: uninstall result preserves reboot-deletion facts and cleanup diagnostics\n";
  return 0;
}
