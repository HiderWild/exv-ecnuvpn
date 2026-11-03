#pragma once

#include <optional>
#include <string>

namespace exv::setup {

enum class Role {
  InstallGui,
  InstallSilent,
  UninstallGui,
  UninstallSilent,
  ElevatedWorker,
};

struct CliOptions {
  Role role{Role::InstallGui};
  std::wstring install_dir;
  bool clear_user_data{false};
  // KI-13 ④：升级链路调卸载链时的保留语义开关——用户数据清理脚本跳过用户数据段
  // （配置/凭据/UI 偏好），缓存/账本照清。全新卸载不传该开关（全清语义）。
  bool preserve_user_data{false};
  bool desktop_shortcut{true};
  bool start_menu{true};
  bool quick_launch{false};
  bool launch_app{true};
  std::wstring elevated_pipe;
  std::wstring elevated_token;
  // Captured before uninstall runas so another administrator cannot redirect cleanup
  // to that administrator's profile through the elevated process environment.
  bool initiating_user_context{false};
  std::wstring initiating_user_sid;
  std::wstring user_profile_root;
  std::wstring local_app_data_root;
  std::wstring roaming_app_data_root;
  std::wstring config_dir;
  std::wstring temp_root;
  bool include_credential_manager{true};
  std::wstring credential_cleanup_note;
};

// Parses wide argv (like wmain). Returns nullopt only on unrecoverable parse errors.
std::optional<CliOptions> ParseCli(int argc, wchar_t **argv);

// Default per-user install directory: %LOCALAPPDATA%\Programs\EXV
std::wstring DefaultInstallDir();

// Existing per-user installs keep their registered location on upgrades.
std::wstring ResolveDefaultInstallDir(const std::wstring &registered_install_dir,
                                      const std::wstring &fallback_dir);

// GUI directory resolution keeps uninstall empty when /D was not supplied so
// the uninstall engine can read the registered custom install directory.
std::wstring ResolveGuiInstallDir(bool uninstall, const CliOptions &options);

// True when executable base name is Uninstall.exe (case-insensitive).
bool IsUninstallExecutableName(const std::wstring &path_or_name);

// Rebuild the uninstall argv passed to ShellExecuteExW("runas"), preserving the
// initiating user's explicit cleanup roots and SID across an alternate-admin UAC.
std::wstring BuildUninstallElevationParameters(const CliOptions &options);

}  // namespace exv::setup
