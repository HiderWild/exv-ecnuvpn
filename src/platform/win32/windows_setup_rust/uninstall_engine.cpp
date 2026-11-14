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

// 停服链（SCM 报 SERVICE_STOPPED / 进程被 terminate）与内核实际回收全部句柄
// 之间存在毫秒到秒级的残余窗。对 ERROR_ACCESS_DENIED(5)/ERROR_SHARING_VIOLATION(32)
// 的删除失败做 500ms×≤10 的有界重试（对齐清理脚本 clear-local-user-config.ps1
// 的锁重试纪律），窗口内成功视同立即成功。聚合上界如实声明：N 个锁定路径串行
// 重试时单次卸载最坏约 N×5 秒。
constexpr int kHandleDrainMaxRetries = 10;
constexpr DWORD kHandleDrainRetryDelayMs = 500;

bool IsHandleDrainRetryError(const std::error_code &ec) {
  return ec.value() == ERROR_ACCESS_DENIED || ec.value() == ERROR_SHARING_VIOLATION;
}

// 句柄迟释放有界重试删除：只再试 fs::remove（500ms×≤10，对齐清理脚本纪律）。
// 重试耗尽后是否登记重启删除由调用方决定（DeleteOrSchedule 一试即登记；
// DeleteInstalledPath 走满重试后登记）。等待调用 Sleep 以便测试桩通过宏替换
// 快进（见 tests/uninstall_result_test.cpp）。
bool RemoveWithHandleDrain(const fs::path &path, std::error_code &ec) {
  bool removed = fs::remove(path, ec);
  int retries = 0;
  while (!removed && retries < kHandleDrainMaxRetries && IsHandleDrainRetryError(ec)) {
    ++retries;
    Sleep(kHandleDrainRetryDelayMs);
    ec.clear();
    removed = fs::remove(path, ec);
  }
  if (!removed && !ec) {
    // remove 返回 false 且无错误码＝路径不存在；防御性核实一次，可能在重试
    // 间隙被他方删除，语义等同删除成功。
    std::error_code present_error;
    const bool still_present = fs::exists(path, present_error);
    if (!present_error && !still_present) {
      return true;
    }
    if (present_error) {
      ec = present_error;
    }
  }
  return removed;
}

// 2026-10-10 用户裁定：安装树内被锁路径不再「保留现场并报错」，改为登记原路径
// 重启删除（成功语义，计数进 scheduled_delete_count，Done 页如实提示部分文件
// 将在重启后移除）。同路径重装的安全性不再靠卸载侧永不登记自缚，而由安装程序
// 铺文件前的 PFRO 清扫（pfro_sweep）撤销登记保证。登记本身失败（通常是非提权
// 或注册表异常）才如实报错。
DeletePathResult DeleteInstalledPath(const fs::path &path) {
  DeletePathResult result;
  std::error_code ec;
  if (!fs::exists(path, ec)) {
    if (!ec) return result;
  } else if (RemoveWithHandleDrain(path, ec)) {
    return result;
  }
  const auto remove_error = ec.value();
  if (MoveFileExW(path.c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT)) {
    result.scheduled = true;
    return result;
  }
  const DWORD schedule_error = GetLastError();
  result.ok = false;
  result.error = L"安装目录内路径未能删除且登记重启删除失败：" + path.wstring() +
                 L"（remove=" + std::to_wstring(remove_error) + L"，schedule=" +
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

DeleteTreeResult DeleteTreeBestEffort(const fs::path &root, bool preserve_user_data) {
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
  bool retained_webview_profile = false;
  for (auto it = fs::recursive_directory_iterator(root, ec); !ec && it != fs::recursive_directory_iterator();
       it.increment(ec)) {
    const bool is_directory = it->is_directory(ec);
    if (ec) {
      break;
    }
    // Older Windows builds kept the login session next to exv-ui.exe. Preserve
    // that exact direct child for an upgrade or a keep-data uninstall; the same
    // path can be reused by a later reinstall.
    if (preserve_user_data && it.depth() == 0 && is_directory &&
        _wcsicmp(it->path().filename().c_str(), L"exv-ui.exe.WebView2") == 0) {
      retained_webview_profile = true;
      it.disable_recursion_pending();
      continue;
    }
    if (is_directory) {
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
      // 2026-10-10 裁定：隔离目录建不起来不再硬失败——活动映像原路径登记重启
      // 删除（DeleteOrSchedule），同路径重装安全由安装侧 PFRO 清扫保证。
      files.erase(running_image_file);
    } else {
      const auto quarantined_image = quarantine_dir / running_image_file->filename();
      if (!MoveFileExW(running_image_file->c_str(), quarantined_image.c_str(), MOVEFILE_WRITE_THROUGH)) {
        // 同上：移动失败不再是错误；活动映像留在原路径登记。
        std::error_code cleanup_error;
        fs::remove(quarantine_dir, cleanup_error);
        files.erase(running_image_file);
      } else {
        *running_image_file = quarantined_image;
        quarantined_image_path = quarantined_image;
        relocated_running_image = true;
      }
    }
  }
  if (found_running_image && !relocated_running_image) {
    // F4（2026-10-10 修复计划 T3）：隔离失败（目录建不起来或移动失败）时活动
    // 映像原路径的重启删除登记必须先于目录循环与根登记——活动映像可能位于
    // 嵌套子目录，父目录条目若先登记会在重启时先执行、因目录非空而删除失败，
    // 留下空壳目录。活动映像删除必然失败（运行中或只读），用一试即登记的
    // DeleteOrSchedule 避免空耗重试窗口。此处一经登记，根处理分支只登记安装根。
    AccumulateDeletion(result, DeleteOrSchedule(current_image));
  }

  for (const auto &f : files) {
    AccumulateDeletion(result, f == quarantined_image_path && relocated_running_image
                                   ? DeleteOrSchedule(f)
                                   : DeleteInstalledPath(f));
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
    AccumulateDeletion(result, DeleteInstalledPath(d));
  }
  if (retained_webview_profile) {
    // The root intentionally remains to retain the old WebView2 profile.
    // Never schedule its deletion: a same-path reinstall may reuse it.
  } else if (found_running_image) {
    // 活动映像仍在或曾在安装树内：隔离成功时映像已在文件循环中按隔离路径
    // 登记；隔离失败时已在上文先于目录循环登记（F4 顺序修正）。此处只登记
    // 安装根（2026-10-10 裁定：不保留现场，登记重启删除；同路径重装冲突由
    // 安装程序铺文件前的 PFRO 清扫撤销登记化解）。
    AccumulateDeletion(result, DeleteInstalledPath(root));
  } else {
    AccumulateDeletion(result, DeleteInstalledPath(root));
  }
  return result;
}

// KI-13 ③：从清理脚本的 stdout 中解析机器可读摘要行
//   EXV-CLEANUP-SUMMARY: scheduled=<n>
// 只接受该精确前缀加十进制数字；n 是脚本对被锁文件成功登记的 MoveFileEx 重启删除
// 数。脚本以 exit 0 报告「全清或部分待重启」（两者都是成功语义），硬失败才非零——
// 卸载器据此把登记数并入 reboot 提示，而不再把被锁文件当成卸载失败。
std::uint32_t ParseCleanupScheduledSummary(const std::string &captured_output) {
  static const char kSummaryPrefix[] = "EXV-CLEANUP-SUMMARY: scheduled=";
  std::uint32_t scheduled = 0;
  std::size_t search_from = 0;
  while (true) {
    const auto line_start = captured_output.find(kSummaryPrefix, search_from);
    if (line_start == std::string::npos) {
      break;
    }
    const auto digits_begin = line_start + sizeof(kSummaryPrefix) - 1;
    const auto digits = captured_output.find_first_not_of("0123456789", digits_begin);
    const auto number = captured_output.substr(
        digits_begin, digits == std::string::npos ? std::string::npos : digits - digits_begin);
    if (!number.empty()) {
      try {
        const auto value = std::stoull(number);
        // 多行摘要理论上不应出现；出现时取最大值而不是累加，避免重复放大。
        scheduled = std::max<std::uint32_t>(
            scheduled, static_cast<std::uint32_t>(std::min<std::uint64_t>(value, 0xFFFFFFFFu)));
      } catch (const std::out_of_range &) {
        // 数值溢出：忽略该行，保持 0——提示性信息不应因畸形输入而失败。
      }
    }
    search_from = digits_begin + number.size();
    if (search_from <= line_start) {
      break;
    }
  }
  return scheduled;
}

// F13（2026-10-10 修复计划 T10，v2 定案）：over-the-shoulder 提权检测。复用
// credential 路径先例——main.cpp ResolveCredentialCleanupScope 已把「发起卸载者
// SID vs 当前提权 SID」的比对结果经 UninstallRequest 携带（GUI 与静默两条入口
// 都转发 include_credential_manager + credential_cleanup_note）：凭据照清即 SID
// 一致；note 以「提权身份与发起卸载者不同」开头即两侧 SID 均已知且不同；
// 「无法确认…」变体是 SID 缺失，不构成确诊错位，维持既有行为。
bool ElevatedIdentityDiffersFromInitiator(const UninstallRequest &request) {
  constexpr wchar_t kMismatchNotePrefix[] = L"提权身份与发起卸载者不同";
  return !request.include_credential_manager &&
         request.credential_cleanup_note.rfind(kMismatchNotePrefix, 0) == 0;
}

// v2 定案砍掉 reg load hive 方案（ACL 还原安全面与复杂度远超收益）：HKCU 系残留
// 不做跨用户清理，结果页如实列出残留项及位置。快捷方式路径已参数化为发起用户
// 根（见 RunUninstall 调用点），根非空时实际可清理；根缺失才计入残留清单。
std::wstring BuildElevatedIdentityResidualNotice(const UninstallRequest &request) {
  const auto elevated_sid = CurrentProcessUserSid();
  std::wstring notice = L"提权身份与发起卸载者不同（当前提权身份为 " +
                        (elevated_sid.empty() ? std::wstring(L"未知") : elevated_sid) +
                        L"），以下用户级内容未能为原用户清理：";
  notice += L"\n- 卸载注册表项：原用户 HKEY_CURRENT_USER\\" + std::wstring(kAppKey) +
            L" 与 HKEY_CURRENT_USER\\" + std::wstring(kUninstallKey);
  notice += L"\n- aTrust:// 用户级协议：原用户 HKEY_CURRENT_USER\\" +
            std::wstring(kUrlSchemeKey);
  if (request.user_profile_root.empty() || request.roaming_app_data_root.empty()) {
    notice += L"\n- 三处快捷方式：未捕获发起用户根，无法定位原用户桌面/开始菜单/"
              L"快速启动，已按当前提权身份的已知文件夹清理";
    notice += L"\n残留内容可待原用户登录后手动删除，或以原用户身份重装本应用后再卸载。";
  } else {
    notice += L"\n三处快捷方式已按发起用户根清理（桌面/开始菜单/快速启动）；若原用户"
              L"已知文件夹被重定向到非标准位置，该处快捷方式仍可能残留。";
    notice += L"\n注册表残留可待原用户登录后手动删除，或以原用户身份重装本应用后再卸载。";
  }
  return notice;
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
    // 2026-10-10 裁定：登记范围已含安装树内被锁路径（安装树外清理脚本登记的
    // 被锁文件、隔离映像、安装树内被锁文件/目录壳），统一如实计数提示——部分
    // 文件被占用，已登记延迟清理，重启后自动移除。属成功语义，不吓唬用户。
    append(L"共 " + std::to_wstring(result.scheduled_delete_count) +
           L" 个被占用的文件或目录已登记延迟清理，将在重启后自动移除。");
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
  const bool preserve_user_data = request.preserve_user_data || !request.clear_user_data;
  // T10（F13）：发起卸载者 SID ≠ 当前提权 SID（over-the-shoulder UAC）时，快捷
  // 方式按发起用户根清理，HKCU 系残留如实写入 notice（成功语义，不判失败）。
  const bool identity_mismatch = ElevatedIdentityDiffersFromInitiator(request);
  if (request.clear_user_data) {
    result.notice = request.credential_cleanup_note;
  }
  if (identity_mismatch) {
    if (!result.notice.empty()) {
      result.notice += L"\n";
    }
    result.notice += BuildElevatedIdentityResidualNotice(request);
  }
  std::wstring errors;
  // KI-13 ③：清理脚本对被锁文件登记的重启删除数（脚本 exit 0 时解析摘要行得到）。
  std::uint32_t cleanup_scheduled = 0;
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

  // KI-13 ⑥ 历代服务漂移治理：C++ 3.3.x 时代的提升助手服务名是 exv-helper
  // （NSIS !define SERVICE_NAME "exv-helper"，载荷在 %ProgramData%\EXV\Helper）。
  // 一路覆盖升级、从未卸载的宿主可能仍注册着该遗留服务；卸载必须干净的义务要求
  // 同步移除它。与 exv-engine 同语义：SCM 查询失败或删除失败都如实计错。
  const auto legacy_service = QueryServicePresence(kLegacyHelperServiceName);
  if (legacy_service == ServicePresence::Error) {
    AppendError(errors, L"无法查询遗留 exv-helper 服务状态");
  } else if (legacy_service == ServicePresence::Installed) {
    if (StopAndDeleteService(kLegacyHelperServiceName)) {
      status(L"遗留 exv-helper 服务已移除", 0.30);
    } else {
      AppendError(errors, L"无法停止并删除遗留 exv-helper 服务");
      status(L"遗留服务移除失败，继续清理文件", 0.30);
    }
  }

  status(L"正在移除快捷方式…", 0.38);
  if (progress) {
    progress->SetStageSpan(0.38, 0.48);
  }
  // T10（F13）：身份错位时传入发起用户根（UAC 前捕获），shortcuts.cpp 据此推导
  // 原用户路径；SID 一致传空根 → 维持当前令牌已知文件夹的既有行为。
  RemoveDesktopShortcut(identity_mismatch ? request.user_profile_root : std::wstring{});
  RemoveStartMenuShortcuts(identity_mismatch ? request.roaming_app_data_root : std::wstring{});
  RemoveQuickLaunchShortcut(identity_mismatch ? request.roaming_app_data_root : std::wstring{});
  status(L"快捷方式已移除", 0.48);

  status(L"正在清理注册表…", 0.52);
  if (progress) {
    progress->SetStageSpan(0.52, 0.60);
  }
  RemoveInstallRegistry();
  // T10（F13）：身份错位时以下两项作用于当前提权身份的 HKCU（提权管理员 hive），
  // 原用户 hive 内的注册不做跨用户清理（v2 定案不做 reg load），残留已由 notice
  // 如实列出。
  // L4（S4）：回退 exv-ui 运行时注册的 aTrust:// 用户级 URL 协议（best-effort，
  // 守卫在 RemoveUrlSchemeRegistration 内：只删指向本安装目录的用户级注册）。
  RemoveUrlSchemeRegistration(install_dir);
  status(L"注册表已清理", 0.60);

  {
    status(preserve_user_data ? L"正在清理运行资源…" : L"正在清除用户数据…", 0.66);
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
      AppendError(errors, L"无法清理运行资源/用户数据：未捕获发起卸载者路径（" + missing_roots +
                              L"），为避免访问提权账户已跳过");
      status(L"清理脚本未运行", 0.78);
    } else if (fs::exists(script)) {
      // Hidden PowerShell only — never elevated Verb RunAs window path.
      std::wstring cmd =
          L"powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File " +
          QuoteCommandLineArgument(script.wstring()) + L" -Force";
      if (request.include_credential_manager) {
        cmd += L" -IncludeCredentialManager";
      }
      if (preserve_user_data) {
        // 升级和普通保留数据卸载都清运行资源，同时保留身份与登录数据。
        cmd += L" -PreserveUserData";
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
      append_path(L"-BrowserDataDir", request.browser_data_dir);
      append_path(L"-TempRoot", request.temp_root);
      // KI-13 (2026-09-28 review F2): 300s budget covers the pathological worst
      // case (AV scanning + per-locked-file retries + legacy tree sweeps); a
      // timeout means the script is STILL working in the background (retries and
      // reboot-delete registration are idempotent and complete on their own) --
      // per the user's ruling this must surface as success, not an error dialog.
      const auto cleanup_result = RunHidden(L"", cmd, 300000);
      // 脚本在硬失败时仍输出已登记的锁文件数量；错误页也须保留待重启事实。
      cleanup_scheduled = ParseCleanupScheduledSummary(cleanup_result.captured_stdout);
      if (cleanup_result.started && cleanup_result.timed_out) {
        // Still running: do not kill it, do not fail the uninstall. Write the
        // diagnostic log for observability and surface a success-side note.
        result.diagnostic_log_path = WriteCleanupDiagnosticLog(cleanup_result, request.temp_root);
        status(L"清理脚本仍在后台完成", 0.78);
      } else if (!cleanup_result.started || cleanup_result.exit_code != 0) {
        // KI-13 ③ 语义边界：脚本 exit 非零只代表硬失败（有限重试与重启删除登记
        // 都没做到）；被锁文件已登记重启删除时脚本以 exit 0 报成功，不再走到这里。
        result.diagnostic_log_path = WriteCleanupDiagnosticLog(cleanup_result, request.temp_root);
        std::wstring cleanup_error = cleanup_result.started
                                         ? L"清理脚本执行失败（exit=" +
                                               std::to_wstring(cleanup_result.exit_code) + L"）"
                                         : L"无法启动清理脚本";
        if (result.diagnostic_log_path.empty()) {
          cleanup_error += L"，且无法写入诊断日志";
        }
        AppendError(errors, cleanup_error);
        status(L"清理脚本失败", 0.78);
      } else {
        // 把脚本登记的重启删除数并入结果：Done 页据此给出「部分文件将在重启后自动
        // 清理」的提示性文案（成功语义，KI-13 ①③）。
        status(preserve_user_data ? L"用户数据已保留，运行资源已清理" : L"用户数据已清除", 0.78);
      }
    } else {
      AppendError(errors, L"未找到清理脚本");
      status(L"清理脚本失败", 0.78);
    }
  }

  status(L"正在删除文件…", 0.82);
  if (progress) {
    progress->SetStageSpan(0.82, 0.96);
  }
  const auto delete_result = DeleteTreeBestEffort(install_dir, preserve_user_data);
  result.scheduled_delete_count = cleanup_scheduled + delete_result.scheduled_delete_count;
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
