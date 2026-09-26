#include "windows_setup_rust/uninstall_engine.hpp"

#include "windows_setup_rust/app_paths.hpp"
#include "windows_setup_rust/elevate.hpp"
#include "windows_setup_rust/process_control.hpp"
#include "windows_setup_rust/registry_install.hpp"
#include "windows_setup_rust/service_control.hpp"
#include "windows_setup_rust/shortcuts.hpp"
#include "windows_setup_rust/util/command_line.hpp"
#include "windows_setup_rust/util/hidden_process.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

namespace exv::setup {
namespace fs = std::filesystem;

namespace {

struct DeleteTreeResult {
  bool ok{true};
  std::uint32_t scheduled_delete_count{0};
  std::wstring error;
};

struct DeletePathResult {
  bool ok{true};
  bool scheduled{false};
  std::wstring error;
};

void AppendError(std::wstring &errors, const std::wstring &error) {
  if (!errors.empty()) {
    errors += L"；";
  }
  errors += error;
}

DeletePathResult DeleteOrSchedule(const fs::path &path) {
  DeletePathResult result;
  std::error_code ec;
  const bool present = fs::exists(path, ec);
  if (ec) {
    result.ok = false;
    result.error = L"无法检查路径：" + path.wstring();
    return result;
  }
  if (!present) {
    return result;
  }
  if (fs::remove(path, ec)) {
    return result;
  }
  if (!ec) {
    const bool still_present = fs::exists(path, ec);
    if (!ec && !still_present) {
      return result;
    }
  }
  const auto remove_error = ec.value();
  if (MoveFileExW(path.c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT)) {
    result.scheduled = true;
    return result;
  }
  const DWORD schedule_error = GetLastError();
  result.ok = false;
  result.error = L"无法删除或登记重启删除：" + path.wstring() + L"（remove=" +
                 std::to_wstring(remove_error) + L"，schedule=" +
                 std::to_wstring(schedule_error) + L"）";
  return result;
}

void AccumulateDeletion(DeleteTreeResult &tree, const DeletePathResult &path) {
  if (path.scheduled) {
    ++tree.scheduled_delete_count;
  }
  if (!path.ok) {
    tree.ok = false;
    AppendError(tree.error, path.error);
  }
}

std::wstring CreateTemporaryLogPath(const std::wstring &preferred_temp_root) {
  wchar_t temp_file[MAX_PATH + 1] = {};
  if (!preferred_temp_root.empty() && preferred_temp_root.size() <= MAX_PATH &&
      GetTempFileNameW(preferred_temp_root.c_str(), L"exv", 0, temp_file) != 0) {
    return temp_file;
  }

  wchar_t process_temp[MAX_PATH + 1] = {};
  const DWORD length = GetTempPathW(MAX_PATH + 1, process_temp);
  if (length == 0 || length > MAX_PATH ||
      GetTempFileNameW(process_temp, L"exv", 0, temp_file) == 0) {
    return {};
  }
  return temp_file;
}

std::wstring WriteCleanupDiagnosticLog(const HiddenProcessResult &cleanup,
                                       const std::wstring &preferred_temp_root) {
  const auto temp_file = CreateTemporaryLogPath(preferred_temp_root);
  if (temp_file.empty()) {
    return {};
  }

  std::ofstream output(fs::path(temp_file), std::ios::binary | std::ios::trunc);
  if (!output) {
    DeleteFileW(temp_file.c_str());
    return {};
  }
  output << "started=" << (cleanup.started ? "true" : "false") << "\r\n"
         << "exit_code=" << cleanup.exit_code << "\r\n"
         << "captured_output:\r\n"
         << cleanup.captured_stdout;
  output.flush();
  if (!output) {
    output.close();
    DeleteFileW(temp_file.c_str());
    return {};
  }
  return temp_file;
}

DeleteTreeResult DeleteTreeBestEffort(const fs::path &root) {
  DeleteTreeResult result;
  std::error_code ec;
  if (!fs::exists(root, ec)) {
    if (ec) {
      result.ok = false;
      result.error = L"无法检查安装目录：" + root.wstring();
    }
    return result;
  }

  // Delete files first deepest-first.
  std::vector<fs::path> files;
  std::vector<fs::path> dirs;
  for (auto it = fs::recursive_directory_iterator(root, ec); !ec && it != fs::recursive_directory_iterator();
       it.increment(ec)) {
    if (it->is_directory(ec)) {
      dirs.push_back(it->path());
    } else {
      files.push_back(it->path());
    }
  }
  if (ec) {
    result.ok = false;
    result.error = L"无法枚举安装目录：" + root.wstring();
    return result;
  }

  for (const auto &f : files) {
    AccumulateDeletion(result, DeleteOrSchedule(f));
  }
  std::sort(dirs.begin(), dirs.end(),
            [](const fs::path &a, const fs::path &b) {
              return a.wstring().size() > b.wstring().size();
            });
  for (const auto &d : dirs) {
    AccumulateDeletion(result, DeleteOrSchedule(d));
  }
  AccumulateDeletion(result, DeleteOrSchedule(root));
  return result;
}

}  // namespace

std::wstring BuildUninstallResultDetail(const UninstallResult &result) {
  std::wstring detail;
  auto append = [&](const std::wstring &line) {
    if (line.empty()) {
      return;
    }
    if (!detail.empty()) {
      detail.push_back(L'\n');
    }
    detail += line;
  };
  // 重启提示优先显示，避免错误正文很长时被页面底部裁掉。
  if (result.reboot_required) {
    append(L"部分文件将在重启后自动删除。");
  }
  append(result.error);
  append(result.notice);
  if (!result.diagnostic_log_path.empty()) {
    append(L"诊断日志：" + result.diagnostic_log_path);
  }
  return detail;
}

UninstallResult RunUninstall(const UninstallRequest &request, ProgressModel *progress) {
  UninstallResult result;
  if (request.clear_user_data) {
    result.notice = request.credential_cleanup_note;
  }
  std::wstring errors;
  // Monotonic milestones 0→1. UI maps target to falling water (1 - t).
  // Skipped work still advances the waterline so each step is visually accounted for.
  double mark = 0.0;
  auto status = [&](const wchar_t *s, double t) {
    mark = std::max(mark, std::clamp(t, 0.0, 1.0));
    if (progress) {
      progress->SetStatus(s);
      progress->SetTarget(mark);
    }
  };

  std::wstring install_dir = request.install_dir;
  if (install_dir.empty()) {
    install_dir = ReadRegisteredInstallDir();
  }
  if (install_dir.empty()) {
    result.error = L"无法确定安装目录";
    return result;
  }

  status(L"正在结束已运行的 EXV…", 0.06);
  if (progress) {
    progress->SetStageSpan(0.06, 0.10);
  }
  StopRunningAppProcesses();
  status(L"已结束运行中的进程", 0.10);

  status(L"正在移除服务…", 0.16);
  if (progress) {
    progress->SetStageSpan(0.16, 0.30);
  }
  // Rust line: the engine service is self-installed by exv-engine --service-install.
  // Uninstall via exv-engine --service-uninstall (engine sits at install_dir root,
  // at the install root, with a direct SCM delete as fallback.
  const auto presence = QueryServicePresence(kEngineServiceName);
  if (presence == ServicePresence::Error) {
    AppendError(errors, L"无法查询 exv-engine 服务状态");
  } else if (presence == ServicePresence::Installed) {
    const auto engine = fs::path(install_dir) / L"exv-engine.exe";
    bool ok = false;
    if (fs::exists(engine)) {
      ok = RunEngineServiceUninstall(engine.wstring());
      if (ok) {
        WaitForServiceRemoved(kEngineServiceName);
      }
    }
    if (!ok || IsServiceInstalled(kEngineServiceName)) {
      ok = StopAndDeleteService(kEngineServiceName);
    }
    if (!ok) {
      AppendError(errors, L"无法停止并删除 exv-engine 服务");
    }
    status(ok ? L"服务已移除" : L"服务移除失败，继续清理文件", 0.30);
  } else {
    status(L"未安装服务，已跳过", 0.30);
  }

  status(L"正在移除快捷方式…", 0.38);
  if (progress) {
    progress->SetStageSpan(0.38, 0.48);
  }
  RemoveDesktopShortcut();
  RemoveStartMenuShortcuts();
  RemoveQuickLaunchShortcut();
  status(L"快捷方式已移除", 0.48);

  status(L"正在清理注册表…", 0.52);
  if (progress) {
    progress->SetStageSpan(0.52, 0.60);
  }
  RemoveInstallRegistry();
  status(L"注册表已清理", 0.60);

  if (request.clear_user_data) {
    status(L"正在清除用户数据…", 0.66);
    if (progress) {
      progress->SetStageSpan(0.66, 0.78);
    }
    std::wstring missing_roots;
    auto require_root = [&](const wchar_t *name, const std::wstring &value) {
      if (value.empty()) {
        if (!missing_roots.empty()) {
          missing_roots += L"、";
        }
        missing_roots += name;
      }
    };
    require_root(L"USERPROFILE", request.user_profile_root);
    require_root(L"LOCALAPPDATA", request.local_app_data_root);
    require_root(L"APPDATA", request.roaming_app_data_root);
    require_root(L"TEMP", request.temp_root);

    const auto script = fs::path(install_dir) / L"support" / L"clear-local-user-config.ps1";
    if (!missing_roots.empty()) {
      AppendError(errors, L"无法清理用户数据：未捕获发起卸载者路径（" + missing_roots +
                              L"），为避免访问提权账户已跳过");
      status(L"用户数据清理失败", 0.78);
    } else if (fs::exists(script)) {
      // Hidden PowerShell only — never elevated Verb RunAs window path.
      std::wstring cmd =
          L"powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File " +
          QuoteCommandLineArgument(script.wstring()) + L" -Force";
      if (request.include_credential_manager) {
        cmd += L" -IncludeCredentialManager";
      }
      auto append_path = [&](const wchar_t *name, const std::wstring &value) {
        if (!value.empty()) {
          cmd += L" ";
          cmd += name;
          cmd += L" ";
          cmd += QuoteCommandLineArgument(value);
        }
      };
      append_path(L"-InstallDir", install_dir);
      append_path(L"-UserProfileRoot", request.user_profile_root);
      append_path(L"-LocalAppDataRoot", request.local_app_data_root);
      append_path(L"-RoamingAppDataRoot", request.roaming_app_data_root);
      append_path(L"-ConfigDir", request.config_dir);
      append_path(L"-TempRoot", request.temp_root);
      const auto cleanup_result = RunHidden(L"", cmd, 120000);
      if (!cleanup_result.started || cleanup_result.exit_code != 0) {
        result.diagnostic_log_path = WriteCleanupDiagnosticLog(cleanup_result, request.temp_root);
        std::wstring cleanup_error = cleanup_result.started
                                         ? L"用户数据清理脚本执行失败（exit=" +
                                               std::to_wstring(cleanup_result.exit_code) + L"）"
                                         : L"无法启动用户数据清理脚本";
        if (result.diagnostic_log_path.empty()) {
          cleanup_error += L"，且无法写入诊断日志";
        }
        AppendError(errors, cleanup_error);
        status(L"用户数据清理失败", 0.78);
      } else {
        status(L"用户数据已清除", 0.78);
      }
    } else {
      AppendError(errors, L"未找到用户数据清理脚本");
      status(L"用户数据清理失败", 0.78);
    }
  } else {
    status(L"保留用户数据", 0.78);
  }

  status(L"正在删除文件…", 0.82);
  if (progress) {
    progress->SetStageSpan(0.82, 0.96);
  }
  const auto delete_result = DeleteTreeBestEffort(install_dir);
  result.scheduled_delete_count = delete_result.scheduled_delete_count;
  result.reboot_required = result.scheduled_delete_count != 0;
  if (!delete_result.ok) {
    AppendError(errors, delete_result.error);
    status(L"文件删除失败", 0.96);
  } else {
    status(L"文件已删除或已登记延迟删除", 0.96);
  }

  if (!errors.empty()) {
    result.error = errors;
    status(L"卸载未完成", 1.0);
    return result;
  }
  status(L"卸载完成", 1.0);
  result.ok = true;
  return result;
}

}  // namespace exv::setup
