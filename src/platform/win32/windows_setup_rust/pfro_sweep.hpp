#pragma once

#include <cstdint>
#include <string>

namespace exv::setup {

// 2026-10-10 用户裁定配套：卸载侧对安装树内被锁路径也登记重启删除（原路径，
// MOVEFILE_DELAY_UNTIL_REBOOT），安装侧在铺文件前默认撤销这些登记——两条合起来
// 既消灭「卸载报错吓唬用户」，又消除陈旧 PendingFileRenameOperations 在同路径
// 重装后重启时误删新文件的历史风险（A2 时代靠卸载侧永不登记来规避，代价是报错）。
struct PfroSweepResult {
  bool ok = true;
  std::wstring error;
  // 被整对撤销的登记数（源+目标为一对）。
  std::uint32_t cancelled_pairs = 0;
  // 顺带直接删除成功的 .exv-uninstall-* 隔离目录数。
  std::uint32_t quarantines_removed = 0;
  // 2026-10-10 修正：安装 GUI 是 asInvoker（非提权），打开 HKLM Session Manager
  // 的写权限会被拒。探测到「确有 EXV 名下登记需要撤销」但当前令牌无写权限时，
  // 返回 ok=true + needs_elevation=true，由调用方经 UAC 提权通道（elevate.cpp）
  // 重跑完整清扫；注册表本身读不了才是失败。
  bool needs_elevation = false;
};

// 读取 HKLM Session Manager 的 PendingFileRenameOperations，逐对撤销 EXV 名下
// 登记并写回（第三方条目逐对原样保留），顺带 best-effort 删除尚存的隔离目录。
// 匹配域：目标安装目录子树（安装即将重建该子树，登记一律撤销）与
// .exv-uninstall-* 隔离目录（删除成功才撤销其登记，删不掉的留给重启兜底）。
// 需要管理员权限；任何注册表读写失败都返回 ok=false（fail-closed：撤销不了
// 的安装子树登记可能在重启时删除新装文件，宁可让安装如实失败）。
PfroSweepResult SweepExvPendingFileRenameOperations(const std::wstring &install_dir);

// 纯判定（供单测）：PFRO 源字符串是否指向 EXV 名下路径。source 形如
// `\??\C:\...` 或带标志前缀的 `*N\??\C:\...`；install_dir 用于子树匹配。
bool IsExvOwnedPfroSource(const wchar_t *source, const std::wstring &install_dir);

// 纯解析（供单测）：剥出源字符串的路径部分（定位 `\??\` 之后到串尾）。
// 无 `\??\` 视为未知格式返回 false，调用方必须原样保留该对（fail-safe）。
bool TryExtractPfroSourcePath(const wchar_t *source, std::wstring &path);

// 路径中任一目录段以 .exv-uninstall- 开头即视为 EXV 卸载隔离目录命名域
// （卸载引擎与清理脚本共用该前缀，后接 pid/guid 保证唯一、不会被新装复用）。
bool IsExvQuarantinePath(const std::wstring &path);

}  // namespace exv::setup
