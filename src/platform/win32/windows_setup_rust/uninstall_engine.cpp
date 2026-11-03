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
#include <aclapi.h>

#include <algorithm>
#include <climits>
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

bool GetCurrentImagePath(fs::path &path, std::wstring &error) {
  std::vector<wchar_t> buffer(32768);
  const DWORD length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
  if (length == 0 || length >= buffer.size()) {
    error = L"无法确定当前卸载器映像路径（error=" + std::to_wstring(GetLastError()) + L"）";
    return false;
  }
  path = fs::path(std::wstring(buffer.data(), length));
  return true;
}

bool SameNormalizedWindowsPath(const fs::path &left, const fs::path &right, bool &same,
                               std::wstring &error) {
  std::error_code ec;
  const auto left_absolute = fs::absolute(left, ec).lexically_normal();
  if (ec) {
    error = L"无法规范化当前卸载器映像路径";
    return false;
  }
  const auto right_absolute = fs::absolute(right, ec).lexically_normal();
  if (ec) {
    error = L"无法规范化安装文件路径：" + right.wstring();
    return false;
  }
  auto left_text = left_absolute.native();
  auto right_text = right_absolute.native();
  std::replace(left_text.begin(), left_text.end(), L'/', L'\\');
  std::replace(right_text.begin(), right_text.end(), L'/', L'\\');
  if (left_text.size() > INT_MAX || right_text.size() > INT_MAX) {
    error = L"卸载器映像路径过长";
    return false;
  }
  const int comparison = CompareStringOrdinal(left_text.data(), static_cast<int>(left_text.size()),
                                              right_text.data(), static_cast<int>(right_text.size()), TRUE);
  if (comparison == 0) {
    error = L"无法比较当前卸载器映像路径";
    return false;
  }
  same = comparison == CSTR_EQUAL;
  return true;
}

bool CreateImageQuarantineDirectory(const fs::path &root, const fs::path &image,
                                   fs::path &directory, std::wstring &error) {
  PSID owner = nullptr;
  PSID group = nullptr;
  PACL dacl = nullptr;
  PSECURITY_DESCRIPTOR descriptor = nullptr;
  const DWORD security_error = GetNamedSecurityInfoW(
      const_cast<LPWSTR>(root.c_str()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION,
      &owner, &group, &dacl, nullptr, &descriptor);
  if (security_error != ERROR_SUCCESS || !descriptor) {
    error = L"无法读取安装目录 ACL，拒绝移动运行中的卸载器（error=" +
            std::to_wstring(security_error) + L"）";
    if (descriptor) LocalFree(descriptor);
    return false;
  }
  if (!SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED)) {
    const DWORD control_error = GetLastError();
    LocalFree(descriptor);
    error = L"无法保护卸载器隔离目录 ACL（error=" + std::to_wstring(control_error) + L"）";
    return false;
  }
  SECURITY_ATTRIBUTES attributes{sizeof(SECURITY_ATTRIBUTES), descriptor, FALSE};
  const auto parent = root.parent_path();
  if (parent.empty()) {
    LocalFree(descriptor);
    error = L"无法在安装目录同卷创建卸载器隔离目录";
    return false;
  }
  wchar_t image_volume[MAX_PATH] = {};
  wchar_t parent_volume[MAX_PATH] = {};
  if (!GetVolumePathNameW(image.c_str(), image_volume, MAX_PATH) ||
      !GetVolumePathNameW(parent.c_str(), parent_volume, MAX_PATH) ||
      _wcsicmp(image_volume, parent_volume) != 0) {
    LocalFree(descriptor);
    error = L"无法确认卸载器隔离目录与安装目录位于同一卷";
    return false;
  }

  static volatile LONG sequence = 0;
  const auto base = L".exv-uninstall-" + std::to_wstring(GetCurrentProcessId()) + L"-" +
                    std::to_wstring(GetTickCount64());
  for (int attempt = 0; attempt < 32; ++attempt) {
    const auto suffix = std::to_wstring(InterlockedIncrement(&sequence));
    const auto candidate = parent / (base + L"-" + suffix);
    if (CreateDirectoryW(candidate.c_str(), &attributes)) {
      directory = candidate;
      LocalFree(descriptor);
      return true;
    }
    const DWORD create_error = GetLastError();
    if (create_error != ERROR_ALREADY_EXISTS) {
      LocalFree(descriptor);
      error = L"无法创建卸载器隔离目录（error=" + std::to_wstring(create_error) + L"）";
      return false;
    }
  }
  LocalFree(descriptor);
  error = L"无法创建唯一的卸载器隔离目录";
  return false;
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

  bool relocated_running_image = false;
  bool found_running_image = false;
  fs::path quarantined_image_path;
  fs::path current_image;
  std::wstring image_path_error;
  if (!GetCurrentImagePath(current_image, image_path_error)) {
    result.ok = false;
    result.error = image_path_error;
    return result;  // Fail closed before deleting anything if self-image identity is unknown.
  }
  auto running_image_file = files.end();
  bool equivalent_alias_found = false;
  for (auto file = files.begin(); file != files.end(); ++file) {
    bool exact_path = false;
    if (!SameNormalizedWindowsPath(current_image, *file, exact_path, image_path_error)) {
      result.ok = false;
      result.error = image_path_error;
      return result;
    }
    if (exact_path) {
      running_image_file = file;
      break;
    }
    std::error_code equivalent_error;
    if (fs::equivalent(*file, current_image, equivalent_error)) {
      equivalent_alias_found = true;
    } else if (equivalent_error) {
      result.ok = false;
      AppendError(result.error, L"无法确认安装文件是否与当前卸载器映像别名：" + file->wstring());
      return result;
    }
  }
  if (running_image_file == files.end() && equivalent_alias_found) {
    result.ok = false;
    result.error = L"安装目录只包含当前卸载器映像的别名，拒绝移动或登记该别名路径";
    return result;
  }
  if (running_image_file != files.end()) {
    found_running_image = true;
    fs::path quarantine_dir;
    if (!CreateImageQuarantineDirectory(root, *running_image_file, quarantine_dir, image_path_error)) {
      result.ok = false;
      AppendError(result.error, image_path_error);
      files.erase(running_image_file);  // Never schedule deletion of the still-running image at its installed path.
    } else {
      const auto quarantined_image = quarantine_dir / running_image_file->filename();
      if (!MoveFileExW(running_image_file->c_str(), quarantined_image.c_str(), MOVEFILE_WRITE_THROUGH)) {
        const DWORD move_error = GetLastError();
        std::error_code cleanup_error;
        fs::remove(quarantine_dir, cleanup_error);
        result.ok = false;
        AppendError(result.error, L"无法将运行中的卸载器移入隔离目录（error=" +
                                      std::to_wstring(move_error) + L"）");
        files.erase(running_image_file);  // Fail closed: never queue the active image's original path.
      } else {
        *running_image_file = quarantined_image;
        quarantined_image_path = quarantined_image;
        relocated_running_image = true;
      }
    }
  }

  for (const auto &f : files) {
    AccumulateDeletion(result, DeleteOrSchedule(f));
    if (relocated_running_image && f == quarantined_image_path) {
      // Always try the exact unique quarantine parent, even if deleting the image
      // succeeded immediately; a missing parent is a successful no-op.
      AccumulateDeletion(result, DeleteOrSchedule(quarantined_image_path.parent_path()));
    }
  }
  std::sort(dirs.begin(), dirs.end(),
            [](const fs::path &a, const fs::path &b) {
              return a.wstring().size() > b.wstring().size();
            });
  for (const auto &d : dirs) {
    AccumulateDeletion(result, DeleteOrSchedule(d));
  }
  if (found_running_image) {
    std::error_code root_error;
    if (fs::remove(root, root_error)) {
      // The installation path is now free for an immediate reinstall.
    } else {
      result.ok = false;
      AppendError(result.error, L"安装目录仍有未删除文件，未登记安装目录本身的重启删除（remove=" +
                                    std::to_wstring(root_error.value()) + L"）");
    }
  } else {
    AccumulateDeletion(result, DeleteOrSchedule(root));
  }
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
