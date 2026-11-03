#include "windows_setup_rust/pfro_sweep.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <algorithm>
#include <chrono>
#include <cwctype>
#include <filesystem>
#include <iterator>
#include <memory>
#include <thread>
#include <vector>

namespace exv::setup {
namespace fs = std::filesystem;

bool TryExtractPfroSourcePath(const wchar_t *source, std::wstring &path) {
  if (source == nullptr) {
    return false;
  }
  // 源条目可带标志前缀（实测有 `*N`、文档有 `!`），路径部分自 `\??\` 起。
  // 不解析标志语义：撤销与保留都以整对为单位，无需理解标志。
  const std::wstring_view text(source);
  constexpr std::wstring_view kMarker = L"\\??\\";
  const auto marker = text.rfind(kMarker);
  if (marker == std::wstring_view::npos) {
    return false;
  }
  path.assign(text.substr(marker + kMarker.size()));
  return !path.empty();
}

bool IsExvQuarantinePath(const std::wstring &path) {
  constexpr wchar_t kQuarantinePrefix[] = L".exv-uninstall-";
  for (const auto &part : fs::path(path)) {
    const std::wstring name = part.wstring();
    if (name.size() >= std::size(kQuarantinePrefix) - 1 &&
        name.compare(0, std::size(kQuarantinePrefix) - 1, kQuarantinePrefix) == 0) {
      return true;
    }
  }
  return false;
}

namespace {

// 大小写不敏感的目录前缀判定（按 `\` 边界，避免 EXV2 之类误命中）。
bool IsPathWithin(const std::wstring &path, const std::wstring &root) {
  if (path.empty() || root.empty()) {
    return false;
  }
  if (path.size() < root.size()) {
    return false;
  }
  if (std::equal(root.begin(), root.end(), path.begin(), [](wchar_t a, wchar_t b) {
        return towlower(a) == towlower(b);
      })) {
    if (path.size() == root.size()) {
      return true;
    }
    // root 尾部斜杠已剥（调用方保证）；剩余部分必须以边界斜杠开头。
    return path[root.size()] == L'\\' || path[root.size()] == L'/';
  }
  return false;
}

// 有界 best-effort 删除：被短暂占用的残留多在几次重试内放开；删不掉不报错
// （隔离目录场景由调用方保留其 PFRO 登记交给重启兜底）。
bool RemoveBestEffortWithRetry(const fs::path &target) {
  constexpr int kAttempts = 4;
  constexpr auto kDelay = std::chrono::milliseconds(200);
  for (int attempt = 0; attempt < kAttempts; ++attempt) {
    std::error_code ec;
    const bool existed = fs::exists(target, ec);
    if (!existed && !ec) {
      return true;
    }
    fs::remove_all(target, ec);
    if (!ec || !fs::exists(target, ec)) {
      std::error_code gone;
      if (!fs::exists(target, gone)) {
        return true;
      }
    }
    std::this_thread::sleep_for(kDelay);
  }
  std::error_code final_check;
  return !fs::exists(target, final_check);
}

std::wstring NormalizeDir(const std::wstring &dir) {
  std::wstring normalized;
  normalized.reserve(dir.size());
  for (const wchar_t c : dir) {
    normalized.push_back(c == L'/' ? L'\\' : c);
  }
  while (normalized.size() > 1 && normalized.back() == L'\\') {
    normalized.pop_back();
  }
  return normalized;
}

// 从登记路径还原其所属隔离目录根（截到 .exv-uninstall-* 段为止）。
bool TryExtractQuarantineRoot(const std::wstring &path, std::wstring &root) {
  constexpr wchar_t kQuarantinePrefix[] = L".exv-uninstall-";
  const fs::path parsed(NormalizeDir(path));
  fs::path accumulated;
  for (const auto &part : parsed) {
    if (!accumulated.empty()) {
      accumulated /= part;
    } else {
      accumulated = part;
    }
    const std::wstring name = part.wstring();
    if (name.size() >= std::size(kQuarantinePrefix) - 1 &&
        name.compare(0, std::size(kQuarantinePrefix) - 1, kQuarantinePrefix) == 0) {
      root = accumulated.wstring();
      return true;
    }
  }
  return false;
}

}  // namespace

bool IsExvOwnedPfroSource(const wchar_t *source, const std::wstring &install_dir) {
  std::wstring path;
  if (!TryExtractPfroSourcePath(source, path)) {
    return false;
  }
  if (IsExvQuarantinePath(path)) {
    return true;
  }
  return IsPathWithin(NormalizeDir(path), NormalizeDir(install_dir));
}

PfroSweepResult SweepExvPendingFileRenameOperations(const std::wstring &install_dir) {
  PfroSweepResult result;
  const std::wstring install_root = NormalizeDir(install_dir);
  if (install_root.empty()) {
    result.ok = false;
    result.error = L"无法清理待重启删除项：安装目录未知";
    return result;
  }

  // 读写打开优先（提权进程可一次完成）；非提权被拒时降级只读——探测与隔离
  // 目录清理不需要写权限，只有登记撤销需要；确需撤销时经 needs_elevation
  // 交由调用方走 UAC 提权通道。竞态说明：孤儿清理脚本只会登记 .exv-uninstall-*
  // 唯一隔离路径，清扫写回之后追加的这类登记对本次安装无害（GUID 目录不会被
  // 新装复用），无需等待或加锁。
  HKEY session_manager = nullptr;
  LONG open_error = RegOpenKeyExW(
      HKEY_LOCAL_MACHINE, L"SYSTEM\\CurrentControlSet\\Control\\Session Manager", 0,
      KEY_QUERY_VALUE | KEY_SET_VALUE, &session_manager);
  if (open_error == ERROR_FILE_NOT_FOUND) {
    return result;  // 键不存在：没有任何待处理登记，无事可做。
  }
  const bool has_write_access = open_error == ERROR_SUCCESS;
  if (open_error == ERROR_ACCESS_DENIED) {
    open_error = RegOpenKeyExW(
        HKEY_LOCAL_MACHINE, L"SYSTEM\\CurrentControlSet\\Control\\Session Manager", 0,
        KEY_QUERY_VALUE, &session_manager);
  }
  if (open_error != ERROR_SUCCESS) {
    result.ok = false;
    result.error = L"无法清理待重启删除项：打开会话管理器注册表失败（error=" +
                   std::to_wstring(open_error) + L"）";
    return result;
  }
  using RegistryKeyGuard = std::unique_ptr<std::remove_pointer_t<HKEY>, decltype(&RegCloseKey)>;
  RegistryKeyGuard key_guard(session_manager, RegCloseKey);

  DWORD type = 0;
  DWORD size_bytes = 0;
  const LONG query_error = RegQueryValueExW(session_manager, L"PendingFileRenameOperations",
                                            nullptr, &type, nullptr, &size_bytes);
  if (query_error == ERROR_FILE_NOT_FOUND) {
    return result;  // 值不存在：无事可做。
  }
  if (query_error != ERROR_SUCCESS || (type != REG_MULTI_SZ && type != REG_SZ)) {
    result.ok = false;
    result.error = L"无法清理待重启删除项：读取 PendingFileRenameOperations 失败（error=" +
                   std::to_wstring(query_error) + L"）";
    return result;
  }
  std::vector<wchar_t> buffer((size_bytes / sizeof(wchar_t)) + 1, L'\0');
  DWORD buffer_bytes = static_cast<DWORD>(buffer.size() * sizeof(wchar_t));
  const LONG read_error = RegQueryValueExW(session_manager, L"PendingFileRenameOperations",
                                           nullptr, &type, reinterpret_cast<LPBYTE>(buffer.data()),
                                           &buffer_bytes);
  if (read_error != ERROR_SUCCESS) {
    result.ok = false;
    result.error = L"无法清理待重启删除项：读取登记内容失败（error=" +
                   std::to_wstring(read_error) + L"）";
    return result;
  }

  // REG_MULTI_SZ → 字符串序列（终止空串停止）。单元素 REG_SZ 视作畸形但完整保留。
  std::vector<std::wstring> entries;
  {
    const std::size_t total = buffer_bytes / sizeof(wchar_t);
    std::size_t cursor = 0;
    while (cursor < total) {
      std::size_t end = cursor;
      while (end < total && buffer[end] != L'\0') {
        ++end;
      }
      if (end == cursor) {
        break;  // 终止空串。
      }
      entries.emplace_back(buffer.data() + cursor, end - cursor);
      cursor = end + 1;
    }
  }

  // 逐对处理（源, 目标）：严格配对是会话管理器的消费语义。未知格式（无 \??\）
  // 与第三方条目一律整对原样保留；EXV 名下按裁定撤销。
  std::vector<std::wstring> kept;
  kept.reserve(entries.size());
  bool dropped_any = false;
  for (std::size_t i = 0; i < entries.size(); i += 2) {
    const bool has_pair = i + 1 < entries.size();
    const std::wstring &source = entries[i];
    std::wstring source_path;
    if (!TryExtractPfroSourcePath(source.c_str(), source_path) ||
        !IsExvOwnedPfroSource(source.c_str(), install_root)) {
      kept.push_back(source);
      if (has_pair) {
        kept.push_back(entries[i + 1]);
      }
      continue;
    }
    if (IsExvQuarantinePath(source_path)) {
      // 隔离目录：删除成功才撤销登记；删不掉的留给重启兜底。
      std::wstring quarantine_root;
      if (TryExtractQuarantineRoot(source_path, quarantine_root) &&
          RemoveBestEffortWithRetry(fs::path(quarantine_root))) {
        ++result.quarantines_removed;
        ++result.cancelled_pairs;
        dropped_any = true;
        continue;
      }
      kept.push_back(source);
      if (has_pair) {
        kept.push_back(entries[i + 1]);
      }
      continue;
    }
    // 目标安装子树：安装即将重建该子树，登记一律撤销；顺带 best-effort 清掉
    // 现存残留（多为上一次卸载留下的空目录壳），失败不影响——铺文件会覆盖。
    std::error_code remove_ec;
    fs::remove_all(fs::path(source_path), remove_ec);
    ++result.cancelled_pairs;
    dropped_any = true;
  }

  if (!dropped_any) {
    return result;
  }
  if (!has_write_access) {
    // 非提权令牌：确有登记需要撤销但本进程写不了注册表——交由调用方提权重跑。
    result.needs_elevation = true;
    return result;
  }

  std::vector<wchar_t> rewritten;
  for (const auto &entry : kept) {
    rewritten.insert(rewritten.end(), entry.begin(), entry.end());
    rewritten.push_back(L'\0');
  }
  // 终止空串（空序列时仅写一个空串）。
  rewritten.push_back(L'\0');

  const LONG write_error = RegSetValueExW(
      session_manager, L"PendingFileRenameOperations", 0, REG_MULTI_SZ,
      reinterpret_cast<const BYTE *>(rewritten.data()),
      static_cast<DWORD>(rewritten.size() * sizeof(wchar_t)));
  if (write_error != ERROR_SUCCESS) {
    result.ok = false;
    result.cancelled_pairs = 0;
    result.quarantines_removed = 0;
    result.error = L"无法清理待重启删除项：写回登记失败（error=" +
                   std::to_wstring(write_error) + L"）。存在指向安装目录的旧登记时"
                   L"继续安装可能在重启后误删新装文件，请以管理员身份重试。";
    return result;
  }
  return result;
}

}  // namespace exv::setup
