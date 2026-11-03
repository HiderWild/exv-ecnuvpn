#include "windows_setup_rust/install_engine.hpp"

#include "windows_setup_rust/elevate.hpp"
#include "windows_setup_rust/payload/archive_reader.hpp"
#include "windows_setup_rust/payload/embedded_payload.hpp"
#include "windows_setup_rust/pfro_sweep.hpp"
#include "windows_setup_rust/process_control.hpp"
#include "windows_setup_rust/registry_install.hpp"
#include "windows_setup_rust/service_control.hpp"
#include "windows_setup_rust/webview2_runtime.hpp"
#include "windows_setup_rust/shortcuts.hpp"
#include "windows_setup_rust/uninstaller_copy.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shellapi.h>

#include <algorithm>
#include <cwctype>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <map>
#include <string_view>
#include <utility>
#include <vector>

namespace exv::setup {
namespace fs = std::filesystem;

namespace {

std::wstring SelfPath() {
  wchar_t path[MAX_PATH] = {};
  GetModuleFileNameW(nullptr, path, MAX_PATH);
  return path;
}

bool LooksLikePe(const fs::path &path) {
  std::ifstream in(path, std::ios::binary);
  if (!in) {
    return false;
  }
  char mz[2] = {};
  in.read(mz, 2);
  return in && mz[0] == 'M' && mz[1] == 'Z';
}

bool CopyDirectoryRecursive(const fs::path &from, const fs::path &to) {
  std::error_code ec;
  fs::create_directories(to, ec);
  if (ec) {
    return false;
  }
  for (auto it = fs::recursive_directory_iterator(from, ec); it != fs::recursive_directory_iterator();
       it.increment(ec)) {
    if (ec) {
      return false;
    }
    const auto rel = fs::relative(it->path(), from, ec);
    if (ec) {
      return false;
    }
    const auto dest = to / rel;
    if (it->is_directory(ec)) {
      fs::create_directories(dest, ec);
    } else if (it->is_regular_file(ec)) {
      fs::create_directories(dest.parent_path(), ec);
      // Existing install: force overwrite (NSIS File /r behavior on reinstall).
      fs::remove(dest, ec);
      ec.clear();
      fs::copy_file(it->path(), dest, fs::copy_options::overwrite_existing, ec);
      if (ec) {
        return false;
      }
    }
    if (ec) {
      return false;
    }
  }
  return !ec;
}

// Rust line: the host resolves wintun.dll from env EXV_RUST_VPN_WINTUN_DLL OR the
// frozen default path %USERPROFILE%\.exv\wintun\wintun\bin\amd64\wintun.dll — there
// is NO sibling fallback. The installer must preplace wintun.dll there so the
// packaged app works without an env var.
bool PreplaceWintunToFrozenPath(const fs::path &install_dir) {
  const fs::path src = install_dir / L"wintun.dll";
  std::error_code ec;
  if (!fs::exists(src, ec)) {
    return false;
  }
  wchar_t profile[MAX_PATH] = {};
  const DWORD n = GetEnvironmentVariableW(L"USERPROFILE", profile, MAX_PATH);
  if (n == 0 || n >= MAX_PATH) {
    return false;
  }
  const fs::path dest =
      fs::path(profile) / L".exv" / L"wintun" / L"wintun" / L"bin" / L"amd64" / L"wintun.dll";
  fs::create_directories(dest.parent_path(), ec);
  if (ec) {
    return false;
  }
  fs::copy_file(src, dest, fs::copy_options::overwrite_existing, ec);
  return !ec;
}

std::wstring Utf8ToWide(std::string_view text) {
  if (text.empty()) {
    return {};
  }
  const int size = MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                                       nullptr, 0);
  if (size <= 0) {
    return L"未知解压错误";
  }
  std::wstring out(static_cast<std::size_t>(size), L'\0');
  MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), out.data(), size);
  return out;
}

void SetExtractionError(std::wstring *error, const std::wstring &value) {
  if (error != nullptr) {
    *error = value;
  }
}

bool ExtractExvpFile(const fs::path &exvp, const fs::path &dest, ProgressModel *progress,
                     double base, double weight, std::wstring *error) {
  std::ifstream in(exvp, std::ios::binary);
  if (!in) {
    SetExtractionError(error, L"无法打开安装包文件");
    return false;
  }
  in.seekg(0, std::ios::end);
  const auto sz = in.tellg();
  if (sz <= 0) {
    SetExtractionError(error, L"安装包为空");
    return false;
  }
  in.seekg(0, std::ios::beg);
  std::vector<std::uint8_t> blob(static_cast<std::size_t>(sz));
  in.read(reinterpret_cast<char *>(blob.data()), sz);
  if (!in) {
    SetExtractionError(error, L"读取安装包失败");
    return false;
  }
  payload::ArchiveView view;
  if (!payload::ParseArchive(blob, view)) {
    SetExtractionError(error, L"安装包格式无效");
    return false;
  }
  std::string err;
  const bool ok = payload::ExtractArchive(
      view, dest, &err, [&](std::uint64_t w, std::uint64_t total) {
        if (progress != nullptr && total > 0) {
          progress->SetStage(base, weight, static_cast<double>(w) / static_cast<double>(total));
        }
      });
  if (!ok) {
    SetExtractionError(error, Utf8ToWide(err));
  }
  return ok;
}

bool ExtractEmbeddedPayload(const fs::path &dest, ProgressModel *progress, double base,
                            double weight, std::wstring *error) {
  std::string err;
  const bool ok = payload::ExtractEmbeddedPayloadTo(
      dest, &err, [&](std::uint64_t w, std::uint64_t total) {
        if (progress != nullptr && total > 0) {
          progress->SetStage(base, weight, static_cast<double>(w) / static_cast<double>(total));
        }
      });
  if (!ok) {
    SetExtractionError(error, Utf8ToWide(err));
  }
  return ok;
}

// Cover installation must release the running LocalSystem engine before the
// archive writes exv-engine.exe. Keep the SCM registration: the existing
// service configuration remains valid because the install path is unchanged.
bool PreInstallServiceMaintenance(ElevatedSession &elevation) {
  return EnsureServiceStopped(elevation);
}

bool LaunchInstalledApp(const std::wstring &install_dir, std::wstring *error) {
  const auto ui = fs::path(install_dir) / L"exv-ui.exe";
  if (!fs::exists(ui)) {
    if (error) {
      *error = L"未找到 exv-ui.exe，无法启动。请确认安装包内容完整。";
    }
    return false;
  }
  if (!LooksLikePe(ui)) {
    if (error) {
      *error = L"exv-ui.exe 不是有效程序（演示包可能只含占位文件）。正式包应包含真实二进制。";
    }
    return false;
  }

  // Prefer CreateProcess with working directory = install dir.
  STARTUPINFOW si{};
  si.cb = sizeof(si);
  PROCESS_INFORMATION pi{};
  std::wstring cmd = L"\"" + ui.wstring() + L"\"";
  std::wstring cwd = install_dir;
  if (CreateProcessW(ui.c_str(), cmd.data(), nullptr, nullptr, FALSE, 0, nullptr, cwd.c_str(), &si,
                     &pi)) {
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    return true;
  }

  // Fallback ShellExecute.
  const HINSTANCE r =
      ShellExecuteW(nullptr, L"open", ui.c_str(), nullptr, install_dir.c_str(), SW_SHOWNORMAL);
  if (reinterpret_cast<INT_PTR>(r) > 32) {
    return true;
  }
  if (error) {
    *error = L"启动 EXV 失败（CreateProcess/ShellExecute）。";
  }
  return false;
}

// --- S3b（2026-10-10 安装侧清单消费，计划 §七 v2）------------------------------
// 解压完成后立即做安装目录 diff：install_manifest.tsv 的 installdir 行是判册，
// 磁盘多余文件按册三族判定——payload/legacy-payload 族（含历代退役行）→删，
// runtime-generated（Uninstall.exe / exv-ui.exe.WebView2）→跳过，不在册→只报告
// 不删（防清单库自身漏项误伤未知文件）。清单缺失/解析失败 fail-closed 拒装，
// 对齐 S3c 卸载链的 fail-closed 语义。本段为纯函数（无宿主副作用），单测直调。

enum class ManifestFileClass {
  kCurrent,  // class∈{payload,support} 且 removed 空：本次包应写的现役文件，保留。
  kStale,    // legacy-payload 或 removed 非空的退役行：陈旧文件，删除。
  kRuntime,  // runtime-generated（或数据类行出现在 installdir）：运行期生成/需
             // 保留，跳过且不报未知（复审 S3b MAJOR-1：常驻噪音排除集）。
};

struct InstallManifestDiff {
  bool ok{false};
  std::wstring error;  // 解析/校验失败原文（fail-closed 文案并入）
  // 精确路径册：小写反斜杠相对路径 → 判族（pipeline=path 行）。
  std::map<std::wstring, ManifestFileClass> exact;
  // 子树册：小写反斜杠相对根 → 判族（pipeline=tree 行，覆盖其全部后代）。
  std::vector<std::pair<std::wstring, ManifestFileClass>> trees;
};

std::wstring LowercaseCopy(std::wstring text) {
  std::transform(text.begin(), text.end(), text.begin(), [](wchar_t ch) {
    return static_cast<wchar_t>(::towlower(static_cast<wint_t>(ch)));
  });
  return text;
}

// 判族：仅消费 scope=installdir 行；其余根族行（userprofile/localappdata/...）
// 属卸载/清理链的目标，安装 diff 不触碰。行内 class 决定族：
//   payload/support + removed 空 → kCurrent；legacy-payload 或 removed 非空 →
//   kStale；runtime-generated → kRuntime；user-data/machine-data 出现在
//   installdir 时按保守语义并入 kRuntime（不删不报，数据保全）。
ManifestFileClass ClassifyManifestRow(std::wstring_view klass, std::wstring_view removed) {
  const bool retired = !removed.empty();
  if (klass == L"legacy-payload") {
    return ManifestFileClass::kStale;
  }
  if (klass == L"payload" || klass == L"support") {
    return retired ? ManifestFileClass::kStale : ManifestFileClass::kCurrent;
  }
  return ManifestFileClass::kRuntime;
}

InstallManifestDiff ParseInstallManifestForDiff(std::string_view text) {
  InstallManifestDiff parsed;
  // UTF-8（可带 BOM）：路径与 class 仅 ASCII/受控字符，按字节切行足够。
  if (text.size() >= 3 && static_cast<unsigned char>(text[0]) == 0xEF &&
      static_cast<unsigned char>(text[1]) == 0xBB &&
      static_cast<unsigned char>(text[2]) == 0xBF) {
    text.remove_prefix(3);
  }
  const std::string kHeader =
      "scope\tpath\tclass\tintroduced\tremoved\tpipeline\tnoreboot\taction\tpurpose";
  bool header_seen = false;
  std::size_t line_no = 0;
  std::size_t pos = 0;
  while (pos < text.size()) {
    const std::size_t newline = text.find('\n', pos);
    const std::size_t end = newline == std::string_view::npos ? text.size() : newline;
    std::string_view raw_line = text.substr(pos, end - pos);
    pos = newline == std::string_view::npos ? text.size() : newline + 1;
    ++line_no;
    if (!raw_line.empty() && raw_line.back() == '\r') {
      raw_line.remove_suffix(1);
    }
    if (raw_line.empty() || raw_line[0] == '#') {
      continue;
    }
    if (!header_seen) {
      if (raw_line != kHeader) {
        parsed.error = L"清单表头不符合九列固定格式（第 " + std::to_wstring(line_no) + L" 行）";
        return parsed;
      }
      header_seen = true;
      continue;
    }
    // 列切分：制表符分隔，恰 9 列（与 S3a/S3c 消费方同一 schema 裁决）。
    std::vector<std::string_view> fields;
    std::size_t field_pos = 0;
    while (true) {
      const std::size_t tab = raw_line.find('\t', field_pos);
      if (tab == std::string_view::npos) {
        fields.push_back(raw_line.substr(field_pos));
        break;
      }
      fields.push_back(raw_line.substr(field_pos, tab - field_pos));
      field_pos = tab + 1;
    }
    if (fields.size() != 9) {
      parsed.error = L"清单行列数不为 9（第 " + std::to_wstring(line_no) + L" 行）";
      return parsed;
    }
    if (fields[0] != "installdir") {
      continue;  // 非安装根族行：卸载/清理链目标，安装 diff 不消费。
    }
    if (fields[1].empty()) {
      parsed.error = L"清单 installdir 行 path 为空（第 " + std::to_wstring(line_no) + L" 行）";
      return parsed;
    }
    std::wstring rel = Utf8ToWide(fields[1]);
    std::replace(rel.begin(), rel.end(), L'/', L'\\');
    if (rel.front() == L'\\') {
      parsed.error = L"清单 installdir 行 path 不得以反斜杠开头（第 " +
                     std::to_wstring(line_no) + L" 行）";
      return parsed;
    }
    rel = LowercaseCopy(std::move(rel));
    const std::wstring klass = Utf8ToWide(fields[2]);
    const std::wstring removed = Utf8ToWide(fields[4]);
    const auto file_class = ClassifyManifestRow(klass, removed);
    if (fields[5] == "tree") {
      for (const auto &tree : parsed.trees) {
        if (tree.first == rel) {
          parsed.error = L"清单 installdir 子树行重复（第 " + std::to_wstring(line_no) + L" 行）";
          return parsed;
        }
      }
      parsed.trees.emplace_back(rel, file_class);
    } else {
      if (!parsed.exact.emplace(rel, file_class).second) {
        parsed.error = L"清单 installdir 行路径重复（第 " + std::to_wstring(line_no) + L" 行）";
        return parsed;
      }
    }
  }
  if (!header_seen) {
    parsed.error = L"清单缺少列表头行";
    return parsed;
  }
  if (parsed.exact.empty() && parsed.trees.empty()) {
    parsed.error = L"清单没有任何 installdir 行";
    return parsed;
  }
  parsed.ok = true;
  return parsed;
}

// 纯判定：disk_keys 为磁盘文件相对安装根的小写反斜杠路径集合。三族输出
// （remove/runtime_skip/unknown）均为同一键形态；current 文件不输出（本安装
// 刚写过，属正常在位）。子树行覆盖其后代：先精确匹配，再取最长子树前缀。
void DecideInstallDirDiff(const InstallManifestDiff &manifest,
                          const std::vector<std::wstring> &disk_keys,
                          std::vector<std::wstring> *remove,
                          std::vector<std::wstring> *runtime_skip,
                          std::vector<std::wstring> *unknown) {
  for (const auto &key : disk_keys) {
    const auto exact_it = manifest.exact.find(key);
    if (exact_it != manifest.exact.end()) {
      if (exact_it->second == ManifestFileClass::kStale) {
        remove->push_back(key);
      } else if (exact_it->second == ManifestFileClass::kRuntime) {
        runtime_skip->push_back(key);
      }
      continue;
    }
    const std::wstring *tree_match = nullptr;
    ManifestFileClass tree_class = ManifestFileClass::kCurrent;
    for (const auto &tree : manifest.trees) {
      const std::wstring &root = tree.first;
      if (key.size() > root.size() + 1 && key.compare(0, root.size(), root) == 0 &&
          key[root.size()] == L'\\') {
        if (tree_match == nullptr || root.size() > tree_match->size()) {
          tree_match = &root;
          tree_class = tree.second;
        }
      }
    }
    if (tree_match != nullptr) {
      if (tree_class == ManifestFileClass::kStale) {
        remove->push_back(key);
      } else if (tree_class == ManifestFileClass::kRuntime) {
        runtime_skip->push_back(key);
      }
      continue;
    }
    unknown->push_back(key);
  }
}

}  // namespace

InstallResult RunInstall(const InstallRequest &request, ProgressModel *progress) {
  InstallResult result;
  result.install_dir = request.install_dir;

  // Discrete monotonic milestones: every step (including skipped work) still advances
  // the waterline so the icon fill tracks real install progress, not wall-clock waiting.
  // preflight 0→0.50（检查旧版本 0.04：无进程→0.33 视为完成；有进程→0.15 结束→0.40；
  // 准备服务 0.42→0.46）、解压 0.50→0.85、finalize 0.85→1.0
  double mark = 0.0;
  auto status = [&](const wchar_t *s, double t) {
    OutputDebugStringW((std::wstring(L"[EXV Setup] ") + s + L"\n").c_str());
    mark = std::max(mark, std::clamp(t, 0.0, 1.0));
    if (progress) {
      progress->SetStatus(s);
      progress->SetTarget(mark);
    }
  };
  auto status_done = [&](const wchar_t *s, double t) {
    // Always publish completion of a step, even when work was skipped.
    status(s, t);
  };

  if (request.install_dir.empty()) {
    result.error = L"empty install dir";
    return result;
  }
  if (!IsWebView2EvergreenRuntimeInstalled()) {
    result.error = kWebView2MissingRuntimeMessage;
    status(result.error.c_str(), 0.0);
    return result;
  }

  if (!CanWriteDirectory(request.install_dir)) {
    result.error = L"install directory is not writable";
    return result;
  }

  // S2（2026-10-10 单次 UAC 提权会话）：停服/清扫/起服（含失败恢复）复用同一
  // runas worker——首个提权步骤前一次 UAC，会话存活至安装结束（析构 quit）；
  // 中途死亡（空闲超时/崩溃）由下一提权步骤按需重建（重建=再弹一次 UAC，
  // launch_count 如实累计；真机单 UAC 人工验收归 W5）。install_dir 经宽字符
  // 命令行随 worker 携带，供 sweep_pfro 消费；对象创建本身零 UAC，未安装
  // 服务且无 PFRO 登记的全新安装不会启动任何 worker。
  std::unique_ptr<ElevatedSession> elevation = CreateElevationSession(request.install_dir);

  // 旧进程清理也会结束 exv-engine；必须在它之前保存精确 SCM 状态并正常停服，
  // 避免把清理后的 Stopped 误当成原始状态，或让 SCM 故障恢复与文件覆盖交叉。
  status(L"正在保存原服务运行状态…", 0.01);
  const auto original_service = QueryServiceRuntimeState();
  if (original_service.state == ServiceRuntimeState::Error) {
    result.error = L"安装前无法读取 exv-engine 服务状态（Win32=" +
                   std::to_wstring(original_service.error_code) + L"），请稍后重试。";
    status(result.error.c_str(), 0.01);
    return result;
  }
  if (original_service.state == ServiceRuntimeState::Other) {
    result.error = L"exv-engine 服务尚未处于稳定的运行或停止状态（SCM=" +
                   std::to_wstring(original_service.scm_state) +
                   L"），请等待服务状态稳定后重试安装。";
    status(result.error.c_str(), 0.01);
    return result;
  }
  const bool restore_running_service = original_service.state == ServiceRuntimeState::Running;
  const bool had_service = original_service.state != ServiceRuntimeState::NotInstalled;
  status(restore_running_service ? L"已保存：原服务运行中，安装成功后恢复运行"
                                 : had_service ? L"已保存：原服务已停止，安装后保持停止"
                                               : L"已保存：原服务未安装，保留按需安装行为", 0.02);

  // S5（2026-10-10 换盘服务迁移，计划 §六 v2）：停服前读 ImagePath 判漂移——
  // 规整（引号/大小写/斜杠方向）后的可执行段 ≠ 本次安装目录的 exv-engine.exe
  // 即换盘安装。此处只读不写：检测失败 fail-closed 拒装（读不了就不该动 SCM，
  // 对齐上方运行态读取的保守语义）；迁移执行放在解压成功后（见下），此前的
  // 失败 return 恢复运行仍用旧路径的完好二进制，不落「安装失败 + 服务指向
  // 不存在路径」三输局。
  const std::wstring expected_engine_path =
      (fs::path(request.install_dir) / L"exv-engine.exe").wstring();
  bool needs_service_migration = false;
  std::wstring current_image_path;  // 原文（含尾参形态），错误文案用
  if (had_service) {
    status(L"正在核对服务安装路径…", 0.03);
    const auto image_path = QueryServiceImagePath();
    if (image_path.presence == ServicePresence::Error) {
      result.error = L"安装前无法读取 exv-engine 服务安装路径（Win32=" +
                     std::to_wstring(image_path.error_code) + L"），请稍后重试。";
      status(result.error.c_str(), 0.03);
      return result;
    }
    if (image_path.presence == ServicePresence::Installed) {
      current_image_path = image_path.binary_path;
      needs_service_migration =
          ServiceImagePathDrifted(image_path.binary_path, expected_engine_path);
    }
    // NotInstalled：与运行态快照竞争间服务被移除——无需迁移，后续按需安装。
    status_done(needs_service_migration ? L"检测到换盘安装，服务将迁移到新目录"
                                        : L"服务安装路径未变化",
                0.03);
  }

  if (!PreInstallServiceMaintenance(*elevation)) {
    result.error = L"无法停止已安装的 exv-engine 服务，请稍后重试。";
    status(result.error.c_str(), 0.02);
    return result;
  }

  // F6（2026-10-10 修复计划 T7）：original_service 为 Running 且停服已执行，此后
  // 任一失败 return 前 best-effort 恢复服务运行，恢复结果并入错误文案——用户不再
  // 落「安装失败 + 服务停止」双输局。非提权安装 GUI 下恢复经同一提权会话执行
  // （S2）：会话存活则不再弹 UAC；会话已死亡则重建=再次请求管理员确认，文案
  // 如实告知。停服本身失败（上面的 return）不尝试恢复：服务状态未受本安装器
  // 改动。
  auto attempt_service_recovery = [&]() {
    if (!restore_running_service) {
      return;
    }
    std::wstring recovery_error;
    const bool recovered = EnsureServiceRunning(*elevation, &recovery_error);
    result.error += recovered ? L"；已尝试恢复服务运行：成功；恢复可能再次请求管理员确认。"
                              : L"；已尝试恢复服务运行：失败；恢复可能再次请求管理员确认。";
  };

  // --- NSIS Section Install preflight ---
  // 检查是否有运行中的旧版本（新增步骤）：没有则直接跳到 33% 视为完成；有则 15%，
  // 结束进程后跳到 40%（关旧实例占进度拉满到 40%）。UI 停滞注入以该满值为蠕动上限。
  status(L"正在检查是否运行旧版本…", 0.04);
  if (progress) {
    progress->SetStageSpan(0.04, 0.40);
  }
  // F5（2026-10-10 修复计划 T3）：杀不掉的他用户会话 EXV 进程名单。不为空时
  // 多半正是它们仍占用安装树文件——名单保留到解压失败 return 点如实上报。
  AppProcessStopReport process_stop_report;
  if (!IsAnyExvProcessRunning()) {
    status_done(L"未发现运行中的旧版本", 0.33);
  } else {
    status(L"发现运行中的旧版本，正在结束…", 0.15);
    process_stop_report = StopRunningAppProcesses();
    status_done(L"已结束运行中的进程", 0.40);
  }

  status(L"正在准备服务组件…", 0.42);
  status_done(had_service ? L"服务已停止，保留安装配置" : L"无需预清理服务", 0.46);

  // 2026-10-10 裁定：铺文件前默认撤销 EXV 名下的重启删除登记（上一次卸载可能
  // 对安装树内被锁路径与隔离目录登记过 MOVEFILE_DELAY_UNTIL_REBOOT）。不撤销
  // 的话，陈旧登记会在本次安装后的重启时误删新装文件。fail-closed：撤销不了就
  // 如实失败，不带着风险继续装。
  // 安装 GUI 是 asInvoker（非提权）：无 EXV 登记时只读探测零打扰；确有登记需
  // 撤销时经 UAC 提权 worker 执行（S2：与服务操作同一会话，不再有独立的
  // -pfro 提权实现）。
  status(L"正在清理待重启删除项…", 0.47);
  auto pfro_sweep = SweepExvPendingFileRenameOperations(request.install_dir);
  if (pfro_sweep.ok && pfro_sweep.needs_elevation) {
    std::wstring sweep_error;
    if (RunElevatedPfroSweep(*elevation, &sweep_error)) {
      pfro_sweep.needs_elevation = false;
    } else {
      pfro_sweep.ok = false;
      pfro_sweep.error =
          L"无法清理待重启删除项（需管理员确认撤销旧登记，否则重启时可能误删新装文件）："
          + sweep_error;
    }
  }
  if (!pfro_sweep.ok) {
    result.error = pfro_sweep.error;
    attempt_service_recovery();
    status(result.error.c_str(), 0.47);
    return result;
  }
  status_done(L"待重启删除项已清理", 0.49);

  // --- Extract / copy (overwrite) ---
  // Extract range: 0.50 → 0.85
  constexpr double kExtractBase = 0.50;
  constexpr double kExtractWeight = 0.35;
  status(L"正在安装文件…", kExtractBase);
  if (progress) {
    progress->SetStageSpan(kExtractBase, kExtractBase + kExtractWeight);
  }
  bool extracted = false;
  std::wstring extraction_error;
  if (request.payload_is_directory && !request.payload_path.empty()) {
    extracted = CopyDirectoryRecursive(request.payload_path, request.install_dir);
    if (progress) {
      progress->SetStage(kExtractBase, kExtractWeight, 1.0);
      mark = std::max(mark, kExtractBase + kExtractWeight);
    }
  } else if (!request.payload_path.empty()) {
    extracted = ExtractExvpFile(request.payload_path, request.install_dir, progress, kExtractBase,
                                kExtractWeight, &extraction_error);
    mark = std::max(mark, kExtractBase + kExtractWeight);
  } else {
    extracted = ExtractEmbeddedPayload(request.install_dir, progress, kExtractBase, kExtractWeight,
                                       &extraction_error);
    mark = std::max(mark, kExtractBase + kExtractWeight);
  }
  if (!extracted) {
    result.error = L"解压安装包失败";
    if (!extraction_error.empty()) {
      result.error += L"：" + extraction_error;
    }
    // F5（2026-10-10 修复计划 T3）：杀不掉的他用户会话 EXV 进程仍在运行时，
    // 解压失败文案必须点名（口径与真因对齐：跨会话进程无法终止，安装器
    // 无权限处理，请其用户退出 EXV 后重试），不再静默。
    if (!process_stop_report.inaccessible_processes.empty()) {
      result.error += L"；其他用户会话的 EXV 进程仍在运行：";
      for (std::size_t i = 0; i < process_stop_report.inaccessible_processes.size(); ++i) {
        if (i != 0) {
          result.error += L"、";
        }
        result.error += process_stop_report.inaccessible_processes[i];
      }
    }
    attempt_service_recovery();
    return result;
  }
  status_done(L"文件安装完成", kExtractBase + kExtractWeight);

  // Sanity: require at least something under install dir.
  std::error_code ec;
  if (!fs::exists(fs::path(request.install_dir), ec)) {
    result.error = L"安装目录在解压后不存在";
    attempt_service_recovery();
    return result;
  }

  // S3b（2026-10-10 安装侧清单消费，计划 §七 v2）：解压完成后立即做安装目录
  // diff——里程碑钉在「解压成功后、服务迁移与卸载器写入前」（复审 S3b
  // MINOR-2）：旧 Uninstall.exe 此刻仍在盘上，由 runtime-generated 排除集跳过。
  // 清单缺失 fail-closed 拒装（对齐 S3c 语义——读不了册就无法区分陈旧与未知，
  // 带着不确定性继续装等于把清单库降级成摆设）。
  status(L"正在核对安装目录…", kExtractBase + kExtractWeight);
  if (progress) {
    progress->SetStageSpan(kExtractBase + kExtractWeight, 0.855);
  }
  const fs::path manifest_file =
      request.manifest_path.empty()
          ? fs::path(request.install_dir) / L"support" / L"install_manifest.tsv"
          : fs::path(request.manifest_path);
  std::ifstream manifest_in(manifest_file, std::ios::binary);
  std::string manifest_bytes((std::istreambuf_iterator<char>(manifest_in)),
                             std::istreambuf_iterator<char>());
  if (!manifest_in && manifest_bytes.empty()) {
    result.error = L"安装清单缺失（" + manifest_file.wstring() +
                   L"），无法核对安装目录，安装未完成。清单应随包注入 "
                   L"support\\install_manifest.tsv；缺失即拒装（fail-closed，与卸载链同语义）。";
    attempt_service_recovery();
    status(result.error.c_str(), kExtractBase + kExtractWeight);
    return result;
  }
  const auto manifest = ParseInstallManifestForDiff(manifest_bytes);
  if (!manifest.ok) {
    result.error = L"安装清单无效（" + manifest_file.wstring() + L"）：" + manifest.error +
                   L"。安装未完成（fail-closed）。";
    attempt_service_recovery();
    status(result.error.c_str(), kExtractBase + kExtractWeight);
    return result;
  }
  {
    // 磁盘文件集：小写键（判定/查册）→ 原大小写相对路径（删除/明细呈现）。
    std::map<std::wstring, std::wstring> disk_files;
    for (auto it = fs::recursive_directory_iterator(
             fs::path(request.install_dir),
             fs::directory_options::skip_permission_denied, ec);
         it != fs::recursive_directory_iterator(); it.increment(ec)) {
      if (ec) {
        break;
      }
      if (!it->is_regular_file(ec)) {
        ec.clear();
        continue;
      }
      std::wstring rel = fs::relative(it->path(), request.install_dir, ec).wstring();
      if (ec || rel.empty()) {
        ec.clear();
        continue;
      }
      std::replace(rel.begin(), rel.end(), L'/', L'\\');
      disk_files[LowercaseCopy(rel)] = rel;
    }
    if (ec) {
      result.error = L"无法枚举安装目录（Win32=" + std::to_wstring(ec.value()) +
                     L"），无法核对陈旧文件，安装未完成。";
      attempt_service_recovery();
      status(result.error.c_str(), kExtractBase + kExtractWeight);
      return result;
    }
    std::vector<std::wstring> disk_keys;
    disk_keys.reserve(disk_files.size());
    for (const auto &entry : disk_files) {
      disk_keys.push_back(entry.first);
    }
    std::vector<std::wstring> stale_keys;
    std::vector<std::wstring> runtime_keys;
    std::vector<std::wstring> unknown_keys;
    DecideInstallDirDiff(manifest, disk_keys, &stale_keys, &runtime_keys, &unknown_keys);
    for (const auto &key : stale_keys) {
      const std::wstring &rel = disk_files.at(key);
      const fs::path target = fs::path(request.install_dir) / rel;
      OutputDebugStringW((L"[EXV Setup] 清单判定陈旧，删除：" + target.wstring() + L"\n").c_str());
      fs::remove(target, ec);
      if (ec) {
        result.error = L"无法删除陈旧文件 " + target.wstring() + L"（Win32=" +
                       std::to_wstring(ec.value()) +
                       L"），安装未完成。请关闭占用该文件的程序后重试。";
        attempt_service_recovery();
        status(result.error.c_str(), kExtractBase + kExtractWeight);
        return result;
      }
      result.cleanup.removed_files.push_back(rel);
      // 删空目录壳：只回收因本次删除而变空的祖先目录（如历代 bin\、webui\ 树），
      // 绝不动仍在承载内容的目录，也绝不动安装根本身。按相对键逐级上溯，非空
      // 即停（fs::remove 对非空目录返回 false）。
      fs::path rel_shell = fs::path(key).parent_path();
      while (!rel_shell.empty()) {
        std::error_code shell_ec;
        if (!fs::remove(fs::path(request.install_dir) / rel_shell, shell_ec)) {
          break;
        }
        rel_shell = rel_shell.parent_path();
      }
    }
    for (const auto &key : unknown_keys) {
      const std::wstring &rel = disk_files.at(key);
      OutputDebugStringW((L"[EXV Setup] 不在册文件（只报告不删除）：" +
                          (fs::path(request.install_dir) / rel).wstring() + L"\n")
                             .c_str());
      result.cleanup.unknown_files.push_back(rel);
    }
    OutputDebugStringW((L"[EXV Setup] 安装目录 diff：删除 " +
                        std::to_wstring(result.cleanup.removed_files.size()) + L"，未知 " +
                        std::to_wstring(result.cleanup.unknown_files.size()) + L"，运行期跳过 " +
                        std::to_wstring(runtime_keys.size()) + L"\n")
                           .c_str());
  }
  status_done(L"安装目录已核对", 0.855);

  // S5（2026-10-10 换盘服务迁移）：解压成功后才执行迁移——新 exv-engine.exe
  // 已就位，此后所有失败 return 的 best-effort 恢复从新路径启动也有完好二进制；
  // 经提权会话 migrate_service OP 承载（ChangeServiceConfigW 需
  // SERVICE_CHANGE_CONFIG，非提权 GUI 拿不到）。fail-closed：变更失败拒装，
  // 旧 SCM 配置未被改写（MigrateServiceImagePath 写前失败）或已按引号惯例
  // 原子换段（ChangeServiceConfigW 单次调用），文案含新旧路径。
  if (needs_service_migration) {
    status(L"正在迁移服务安装路径…", 0.86);
    std::wstring migrate_error;
    if (!EnsureServiceImagePathMigrated(*elevation, expected_engine_path, &migrate_error)) {
      result.error = L"无法迁移 exv-engine 服务安装路径（旧：" + current_image_path +
                     L"；新：" + expected_engine_path + L"）：" + migrate_error + L"。安装未完成。";
      attempt_service_recovery();
      status(result.error.c_str(), 0.86);
      return result;
    }
    status_done(L"服务安装路径已迁移到新目录", 0.86);
  } else {
    status_done(had_service ? L"服务安装路径无漂移，跳过迁移" : L"服务未安装，跳过迁移", 0.86);
  }

  status(L"正在写入卸载程序…", 0.87);
  if (progress) {
    progress->SetStageSpan(0.87, 0.90);
  }
  const auto uninstaller = WriteUninstallerCopy(SelfPath(), request.install_dir);
  if (!uninstaller.ok) {
    result.error = uninstaller.error;
    attempt_service_recovery();
    return result;
  }
  status_done(L"卸载程序已就绪", 0.90);

  status(L"正在更新注册表…", 0.91);
  if (progress) {
    progress->SetStageSpan(0.91, 0.93);
  }
  const auto uninstall = (fs::path(request.install_dir) / L"Uninstall.exe").wstring();
  const std::wstring version =
      request.app_version.empty() ? std::wstring(L"0.0.0") : request.app_version;
  if (!WriteInstallRegistry(request.install_dir, version, uninstall)) {
    result.error = L"写入卸载注册表失败";
    attempt_service_recovery();
    return result;
  }
  status_done(L"注册表已更新", 0.93);

  // Rust line: preplace wintun.dll to the frozen default path so the app finds it
  // without EXV_RUST_VPN_WINTUN_DLL. 缺失 DLL 保持可跳过；存在但写入失败必须上报。
  status(L"正在配置 wintun 驱动…", 0.94);
  const bool has_wintun = fs::exists(fs::path(request.install_dir) / L"wintun.dll", ec);
  if (ec) {
    result.error = L"无法检查安装目录中的 wintun 驱动";
    attempt_service_recovery();
    return result;
  }
  const bool wintun_placed = has_wintun && PreplaceWintunToFrozenPath(request.install_dir);
  if (has_wintun && !wintun_placed) {
    result.error = L"wintun 驱动写入失败，请重试安装";
    attempt_service_recovery();
    return result;
  }
  status_done(wintun_placed ? L"wintun 驱动已就位" : L"wintun 未随包携带，跳过", 0.94);

  // NSIS always creates Start Menu during Install section.
  status(L"正在创建开始菜单快捷方式…", 0.95);
  if (progress) {
    progress->SetStageSpan(0.95, 0.96);
  }
  if (!CreateStartMenuShortcuts(request.install_dir)) {
    result.error = L"创建开始菜单快捷方式失败";
    attempt_service_recovery();
    return result;
  }
  status_done(L"开始菜单快捷方式已创建", 0.96);

  // Optional finish-page defaults when silent path requests them immediately.
  if (request.create_desktop_shortcut) {
    status(L"正在创建桌面快捷方式…", 0.97);
    if (!CreateDesktopShortcut(request.install_dir)) {
      result.error = L"创建桌面快捷方式失败";
      attempt_service_recovery();
      return result;
    }
  } else {
    status_done(L"跳过桌面快捷方式", 0.97);
  }
  if (request.create_quick_launch) {
    status(L"正在创建快速启动…", 0.98);
    if (!CreateQuickLaunchShortcut(request.install_dir)) {
      result.error = L"创建快速启动失败";
      attempt_service_recovery();
      return result;
    }
  } else {
    status_done(L"跳过快速启动", 0.98);
  }

  // 成功路径：所有必要文件和收尾都成功后才恢复运行（避免启动半覆盖的二进制）。
  // 失败路径不再停留于停服状态：T7/F6 已在各失败 return 点 best-effort 恢复原
  // Running 服务，恢复结果并入错误文案。
  if (restore_running_service) {
    status(L"正在恢复原服务运行状态…", 0.985);
    if (progress) {
      progress->SetStageSpan(0.985, 0.99);
    }
    std::wstring service_error;
    if (!EnsureServiceRunning(*elevation, &service_error)) {
      result.error = L"文件已安装，但恢复 exv-engine 服务失败：" + service_error +
                     L"。安装未完成，请重试安装或检查服务日志。";
      status(result.error.c_str(), 0.985);
      return result;
    }
    status_done(L"原服务已恢复运行", 0.99);
  } else {
    status_done(had_service ? L"服务保持停止" : L"服务保留按需安装", 0.99);
  }

  // S2：成功路径显式收尾会话（quit + 回收）；各失败 return 由析构兜底。
  elevation->Shutdown();

  if (request.launch_app) {
    status(L"正在启动 EXV…", 0.995);
    if (progress) {
      progress->SetStageSpan(0.995, 1.0);
    }
    std::wstring launch_err;
    if (!LaunchInstalledApp(request.install_dir, &launch_err)) {
      // Do not fail the whole install just because launch failed (file may be a demo stub).
      if (progress) {
        progress->SetStatus(launch_err.empty() ? L"安装完成（启动已跳过）"
                                               : launch_err.c_str());
      }
    }
  }

  status(L"安装完成", 1.0);
  result.ok = true;
  return result;
}

bool LaunchExvUi(const std::wstring &install_dir, std::wstring *error) {
  return LaunchInstalledApp(install_dir, error);
}

}  // namespace exv::setup
