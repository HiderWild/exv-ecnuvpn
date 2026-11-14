#include "windows_setup_rust/cli.hpp"

#include "windows_setup_rust/app_paths.hpp"
#include "windows_setup_rust/registry_install.hpp"
#include "windows_setup_rust/util/command_line.hpp"

#include <algorithm>
#include <cctype>
#include <cwchar>

namespace exv::setup {
namespace {

std::wstring ToLowerAscii(std::wstring value) {
  for (auto &ch : value) {
    if (ch >= L'A' && ch <= L'Z') {
      ch = static_cast<wchar_t>(ch - L'A' + L'a');
    }
  }
  return value;
}

std::wstring FileNameOf(const std::wstring &path) {
  const auto pos = path.find_last_of(L"\\/");
  if (pos == std::wstring::npos) {
    return path;
  }
  return path.substr(pos + 1);
}

bool StartsWithIgnoreCase(const std::wstring &value, const wchar_t *prefix) {
  const auto p = ToLowerAscii(value);
  const auto pref = ToLowerAscii(prefix);
  return p.compare(0, pref.size(), pref) == 0;
}

bool EqualsIgnoreCase(const std::wstring &a, const wchar_t *b) {
  return ToLowerAscii(a) == ToLowerAscii(b);
}

}  // namespace

bool IsUninstallExecutableName(const std::wstring &path_or_name) {
  return EqualsIgnoreCase(FileNameOf(path_or_name), L"Uninstall.exe");
}

std::wstring ResolveDefaultInstallDir(const std::wstring &registered_install_dir,
                                      const std::wstring &fallback_dir) {
  return registered_install_dir.empty() ? fallback_dir : registered_install_dir;
}

std::wstring DefaultInstallDir() {
  return ResolveDefaultInstallDir(ReadRegisteredInstallDir(), DefaultPerUserInstallDir());
}

std::wstring ResolveGuiInstallDir(bool uninstall, const CliOptions &options) {
  if (uninstall) {
    return options.install_dir;
  }
  return options.install_dir.empty() ? DefaultInstallDir() : options.install_dir;
}

std::wstring BuildUninstallElevationParameters(const CliOptions &options) {
  std::wstring parameters;
  auto append_flag = [&](const wchar_t *flag) {
    if (!parameters.empty()) {
      parameters.push_back(L' ');
    }
    parameters += flag;
  };
  auto append_value = [&](const wchar_t *flag, const std::wstring &value) {
    if (value.empty()) {
      return;
    }
    append_flag(flag);
    parameters.push_back(L' ');
    parameters += QuoteCommandLineArgument(value);
  };

  // S1（2026-10-10 静默移除）：不再生成 /S——卸载提权往返一律 GUI 形态。
  append_flag(L"/uninstall");
  if (options.clear_user_data) {
    append_flag(L"/clear-user-data");
  }
  if (options.preserve_user_data) {
    // KI-13 ④：升级语义开关必须穿过 UAC 往返存活。
    append_flag(L"/preserve-user-data");
  }
  append_value(L"/D", options.install_dir);
  if (options.initiating_user_context) {
    append_flag(L"--initiating-user-context");
  }
  append_value(L"--initiating-user-sid", options.initiating_user_sid);
  append_value(L"--user-profile-root", options.user_profile_root);
  append_value(L"--local-app-data-root", options.local_app_data_root);
  append_value(L"--roaming-app-data-root", options.roaming_app_data_root);
  append_value(L"--config-dir", options.config_dir);
  append_value(L"--browser-data-dir", options.browser_data_dir);
  append_value(L"--temp-root", options.temp_root);
  return parameters;
}

CliParseResult ParseCli(int argc, wchar_t **argv) {
  CliParseResult result;
  if (argc < 0 || argv == nullptr) {
    return result;  // Fatal：argv 不可用（既有早退失败语义，退出码 2）。
  }

  CliOptions opts;
  bool uninstall = false;
  bool elevated = false;

  if (argc >= 1 && argv[0] != nullptr && IsUninstallExecutableName(argv[0])) {
    uninstall = true;
  }

  // S1 严格白名单（fail-closed）：按角色分组记账，冲突在解析完成后判定，
  // 违规时报告命令行顺序中的首个白名单外参数。
  struct GroupFirstArg {
    int index = -1;
    std::wstring arg;
  };
  GroupFirstArg first_uninstall_only;  // 卸载器专属旗标（安装器/worker 白名单外）
  GroupFirstArg first_worker_only;     // 提权 worker 专属旗标（卸载器白名单外）
  GroupFirstArg first_install_dir;     // /D（worker 白名单外）
  auto note = [](GroupFirstArg &slot, int index, const std::wstring &arg) {
    if (slot.index < 0) {
      slot.index = index;
      slot.arg = arg;
    }
  };
  auto reject_with = [&](const GroupFirstArg &slot) {
    result.status = CliParseStatus::InvalidArgument;
    result.rejected_argument = slot.arg;
    return result;
  };
  auto reject_earliest = [&](const GroupFirstArg &a, const GroupFirstArg &b) {
    if (a.index >= 0 && (b.index < 0 || a.index < b.index)) {
      return reject_with(a);
    }
    if (b.index >= 0) {
      return reject_with(b);
    }
    return result;  // 不可达（调用点已保证至少一个存在）。
  };

  for (int i = 1; i < argc; ++i) {
    if (argv[i] == nullptr) {
      continue;
    }
    const std::wstring arg = argv[i];
    if (arg.empty()) {
      continue;
    }

    if (EqualsIgnoreCase(arg, L"/uninstall") || EqualsIgnoreCase(arg, L"--uninstall")) {
      uninstall = true;
      note(first_uninstall_only, i, arg);
      continue;
    }
    if (EqualsIgnoreCase(arg, L"/clear-user-data") ||
        EqualsIgnoreCase(arg, L"--clear-user-data")) {
      opts.clear_user_data = true;
      note(first_uninstall_only, i, arg);
      continue;
    }
    if (EqualsIgnoreCase(arg, L"/preserve-user-data") ||
        EqualsIgnoreCase(arg, L"--preserve-user-data")) {
      // KI-13 ④：升级语义开关必须穿过 UAC 往返存活。
      opts.preserve_user_data = true;
      note(first_uninstall_only, i, arg);
      continue;
    }
    if (EqualsIgnoreCase(arg, L"--initiating-user-context")) {
      opts.initiating_user_context = true;
      note(first_uninstall_only, i, arg);
      continue;
    }
    auto consume_value = [&](const wchar_t *name, std::wstring &destination) {
      if (!EqualsIgnoreCase(arg, name)) {
        return 0;
      }
      if (i + 1 >= argc || argv[i + 1] == nullptr) {
        return -1;
      }
      destination = argv[++i];
      return 1;
    };
    int consumed = consume_value(L"--initiating-user-sid", opts.initiating_user_sid);
    if (consumed < 0) return result;
    if (consumed > 0) {
      note(first_uninstall_only, i - 1, arg);
      continue;
    }
    consumed = consume_value(L"--user-profile-root", opts.user_profile_root);
    if (consumed < 0) return result;
    if (consumed > 0) {
      note(first_uninstall_only, i - 1, arg);
      continue;
    }
    consumed = consume_value(L"--local-app-data-root", opts.local_app_data_root);
    if (consumed < 0) return result;
    if (consumed > 0) {
      note(first_uninstall_only, i - 1, arg);
      continue;
    }
    consumed = consume_value(L"--roaming-app-data-root", opts.roaming_app_data_root);
    if (consumed < 0) return result;
    if (consumed > 0) {
      note(first_uninstall_only, i - 1, arg);
      continue;
    }
    consumed = consume_value(L"--config-dir", opts.config_dir);
    if (consumed < 0) return result;
    if (consumed > 0) {
      note(first_uninstall_only, i - 1, arg);
      continue;
    }
    consumed = consume_value(L"--browser-data-dir", opts.browser_data_dir);
    if (consumed < 0) return result;
    if (consumed > 0) {
      note(first_uninstall_only, i - 1, arg);
      continue;
    }
    consumed = consume_value(L"--temp-root", opts.temp_root);
    if (consumed < 0) return result;
    if (consumed > 0) {
      note(first_uninstall_only, i - 1, arg);
      continue;
    }
    if (EqualsIgnoreCase(arg, L"--elevated-worker") || EqualsIgnoreCase(arg, L"/elevated-worker")) {
      elevated = true;
      note(first_worker_only, i, arg);
      continue;
    }
    if (EqualsIgnoreCase(arg, L"--pipe") || EqualsIgnoreCase(arg, L"/pipe")) {
      if (i + 1 >= argc || argv[i + 1] == nullptr) {
        return result;
      }
      opts.elevated_pipe = argv[++i];
      note(first_worker_only, i - 1, arg);
      continue;
    }
    if (StartsWithIgnoreCase(arg, L"--pipe=")) {
      opts.elevated_pipe = arg.substr(7);
      note(first_worker_only, i, arg);
      continue;
    }
    if (EqualsIgnoreCase(arg, L"--token") || EqualsIgnoreCase(arg, L"/token")) {
      if (i + 1 >= argc || argv[i + 1] == nullptr) {
        return result;
      }
      opts.elevated_token = argv[++i];
      note(first_worker_only, i - 1, arg);
      continue;
    }
    if (StartsWithIgnoreCase(arg, L"--token=")) {
      opts.elevated_token = arg.substr(8);
      note(first_worker_only, i, arg);
      continue;
    }
    if (EqualsIgnoreCase(arg, L"--pfro-sweep-dir") || EqualsIgnoreCase(arg, L"/pfro-sweep-dir")) {
      if (i + 1 >= argc || argv[i + 1] == nullptr) {
        return result;
      }
      opts.elevated_pfro_sweep_dir = argv[++i];
      note(first_worker_only, i - 1, arg);
      continue;
    }
    if (StartsWithIgnoreCase(arg, L"--pfro-sweep-dir=")) {
      opts.elevated_pfro_sweep_dir = arg.substr(17);
      note(first_worker_only, i, arg);
      continue;
    }

    // NSIS-compatible /D=path or /D path
    if (StartsWithIgnoreCase(arg, L"/D=")) {
      opts.install_dir = arg.substr(3);
      note(first_install_dir, i, arg);
      continue;
    }
    if (EqualsIgnoreCase(arg, L"/D")) {
      if (i + 1 >= argc || argv[i + 1] == nullptr) {
        return result;
      }
      opts.install_dir = argv[++i];
      note(first_install_dir, i - 1, arg);
      continue;
    }

    // S1 严格白名单（fail-closed）：白名单外参数——含 /S、/silent、--silent、
    // /desktop-shortcut= 等布尔旗标覆写与任意位置参数——一律 InvalidArgument
    //（退出码 3，无任何 UI；CLI 发起时回一行文字提示）。
    result.status = CliParseStatus::InvalidArgument;
    result.rejected_argument = arg;
    return result;
  }

  if (elevated) {
    // 提权 worker 白名单 = 四个内部旗标（elevate.cpp 生成侧），不得混入
    // 卸载器旗标或 /D。
    if (first_uninstall_only.index >= 0 || first_install_dir.index >= 0) {
      return reject_earliest(first_uninstall_only, first_install_dir);
    }
    if (opts.elevated_pipe.empty() || opts.elevated_token.empty()) {
      return result;  // Fatal：worker 缺管道/令牌（既有早退失败语义，退出码 2）。
    }
    opts.role = Role::ElevatedWorker;
  } else if (uninstall) {
    // 卸载器白名单 = /uninstall、/D、/clear-user-data、/preserve-user-data 与
    // runas 往返七旗标（BuildUninstallElevationParameters 生成侧完备枚举）。
    if (first_worker_only.index >= 0) {
      return reject_with(first_worker_only);
    }
    opts.role = Role::UninstallGui;
  } else {
    // 安装器白名单 = 无参数、/D、四个内部提权旗标（独立传入无害）。
    if (first_uninstall_only.index >= 0) {
      return reject_with(first_uninstall_only);
    }
    opts.role = Role::InstallGui;
  }

  if (opts.install_dir.empty() && opts.role == Role::InstallGui) {
    opts.install_dir = DefaultInstallDir();
  }

  result.status = CliParseStatus::Ok;
  result.options = opts;
  return result;
}

}  // namespace exv::setup
