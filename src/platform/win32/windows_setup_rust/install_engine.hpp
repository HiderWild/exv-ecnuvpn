#pragma once

#include "windows_setup_rust/cli.hpp"
#include "windows_setup_rust/progress_model.hpp"

#include <string>
#include <vector>

namespace exv::setup {

struct InstallRequest {
  std::wstring install_dir;
  std::wstring app_version{L"0.0.0"};
  bool create_desktop_shortcut{true};
  bool create_start_menu{true};
  bool create_quick_launch{false};
  bool launch_app{false};  // finish page may launch after success
  // When empty, extract embedded payload resource. When set, treat as EXVP file or package dir.
  std::wstring payload_path;
  // If true, payload_path is a directory to copy (dev/test), not EXVP.
  bool payload_is_directory{false};
  // S3b（2026-10-10 安装侧清单消费，计划 §七 v2）：install_manifest.tsv 的显式路径
  // ——S3a 确立的 -ManifestPath 约定在安装器侧的形态（测试/开发态注入）。为空时
  // 探测 <install_dir>\support\install_manifest.tsv（打包态随包注入，解压后即在位）。
  // 两处都没有视为清单缺失，fail-closed 拒装（对齐 S3c 语义）。
  std::wstring manifest_path;
};

// S3b：解压后安装目录 diff 的清理明细（记入 InstallResult，结果页/日志呈现）。
struct InstallCleanupDetail {
  // 按册删除的陈旧文件（payload/legacy-payload 族，含历代退役行）——相对安装根
  // 的反斜杠路径，保持磁盘原大小写。
  std::vector<std::wstring> removed_files;
  // 不在册的磁盘文件（相对路径）：只报告不删除——防清单库自身漏项误伤未知文件。
  std::vector<std::wstring> unknown_files;

  bool Empty() const { return removed_files.empty() && unknown_files.empty(); }
};

struct InstallResult {
  bool ok{false};
  std::wstring error;
  std::wstring install_dir;
  // S3b：本次安装按册清理与未知文件报告（成功路径填充；失败路径以 error 为准）。
  InstallCleanupDetail cleanup;
};

InstallResult RunInstall(const InstallRequest &request, ProgressModel *progress = nullptr);

// Copy self to install_dir\Uninstall.exe
bool WriteUninstallerCopy(const std::wstring &install_dir);

// Launch $INSTDIR\exv-ui.exe (validates PE). Used by finish page.
bool LaunchExvUi(const std::wstring &install_dir, std::wstring *error = nullptr);

}  // namespace exv::setup
