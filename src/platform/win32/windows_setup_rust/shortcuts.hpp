#pragma once

#include <string>

namespace exv::setup {

bool CreateShortcut(const std::wstring &link_path,
                    const std::wstring &target_path,
                    const std::wstring &icon_path = {},
                    const std::wstring &working_dir = {});

bool CreateStartMenuShortcuts(const std::wstring &install_dir);
bool CreateDesktopShortcut(const std::wstring &install_dir);
bool CreateQuickLaunchShortcut(const std::wstring &install_dir);

// T10（F13，2026-10-10 修复计划 v2 定案）：卸载路径按发起用户根参数化。根为空
// 时回退当前令牌的已知文件夹（SID 一致的常规卸载保持原行为，含桌面重定向用户）；
// 根非空（over-the-shoulder 提权时 UAC 前捕获的原用户根）时按标准目录布局推导，
// 替代当前提权令牌的 SHGetKnownFolderPath 结果——后者指向提权管理员的目录，
// 会清错人并把快捷方式留给原用户。安装侧创建路径维持现状（不参数化）。
bool RemoveDesktopShortcut(const std::wstring &user_profile_root = {});
bool RemoveStartMenuShortcuts(const std::wstring &roaming_app_data_root = {});
bool RemoveQuickLaunchShortcut(const std::wstring &roaming_app_data_root = {});

}  // namespace exv::setup
