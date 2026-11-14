#pragma once

#include <string>

namespace exv::setup {

// 退出码约定（wWinMain 成文，S1 2026-10-10 静默移除后）：
//   0 成功；
//   1 安装/卸载执行失败；
//   2 既有早退失败（CommandLineToArgvW 失败、白名单旗标缺值等不可恢复解析
//     错误、runas/UAC 失败、角色分派落空）；
//   3 参数非法（白名单外参数，含 /S、/silent、布尔旗标覆写）——无任何 UI，
//     CLI 发起时向父控制台回一行文字提示。
enum class Role {
  InstallGui,
  UninstallGui,
  ElevatedWorker,
};

struct CliOptions {
  Role role{Role::InstallGui};
  std::wstring install_dir;
  bool clear_user_data{false};
  // KI-13 ④：升级链路调卸载链时的保留语义开关——用户数据清理脚本跳过用户数据段
  // （配置/凭据/UI 偏好），缓存/账本照清。全新卸载不传该开关（全清语义）。
  bool preserve_user_data{false};
  // GUI 选项缺省值（S1 后不再有 /desktop-shortcut= 等 CLI 布尔旗标覆写，
  // 白名单外参数一律以退出码 3 拒绝）。
  bool desktop_shortcut{true};
  bool start_menu{true};
  bool quick_launch{false};
  bool launch_app{true};
  std::wstring elevated_pipe;
  std::wstring elevated_token;
  std::wstring elevated_pfro_sweep_dir;
  // Captured before uninstall runas so another administrator cannot redirect cleanup
  // to that administrator's profile through the elevated process environment.
  bool initiating_user_context{false};
  std::wstring initiating_user_sid;
  std::wstring user_profile_root;
  std::wstring local_app_data_root;
  std::wstring roaming_app_data_root;
  std::wstring config_dir;
  std::wstring browser_data_dir;
  std::wstring temp_root;
  bool include_credential_manager{true};
  std::wstring credential_cleanup_note;
};

// 解析结果（S1 严格白名单）：
//   Ok               解析成功，options 有效；
//   InvalidArgument  白名单外参数（含 /S）→ 退出码 3，rejected_argument 记录
//                    首个违规参数（CLI 文字提示用）；
//   Fatal            不可恢复解析错误（argv 缺失、白名单旗标缺值）→ 退出码 2
//                    （既有早退失败语义，不动）。
enum class CliParseStatus {
  Ok,
  InvalidArgument,
  Fatal,
};

struct CliParseResult {
  CliParseStatus status{CliParseStatus::Fatal};
  CliOptions options;
  std::wstring rejected_argument;
};

// 严格白名单解析（fail-closed，S1 2026-10-10 静默移除后唯一入口规则）：
//   安装器（映像名非 Uninstall.exe 且无 /uninstall）：无参数、/D=<目录>、
//     内部提权旗标 --elevated-worker --pipe --token --pfro-sweep-dir
//     （提权 worker 子进程即本映像；独立传入无害：无有效管道即退）。
//   卸载器（映像名为 Uninstall.exe 或带 /uninstall）：/uninstall、/D=<目录>、
//     /clear-user-data、/preserve-user-data、runas 往返用户上下文旗标
//     --initiating-user-context --initiating-user-sid --user-profile-root
//     --local-app-data-root --roaming-app-data-root --config-dir --browser-data-dir --temp-root
//     （完备枚举以 BuildUninstallElevationParameters 生成侧为准）。
//   白名单外参数（含 /S、/silent、布尔旗标、位置参数）一律 InvalidArgument。
CliParseResult ParseCli(int argc, wchar_t **argv);

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
