#pragma once

#include <string>

namespace exv::setup {

inline constexpr wchar_t kUninstallKey[] =
    L"Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\EXV";
inline constexpr wchar_t kAppKey[] = L"Software\\EXV";
// L4（S4）：exv-ui 运行时注册的 `aTrust://` 用户级 URL 协议键（atrust_scheme.rs）。
inline constexpr wchar_t kUrlSchemeKey[] = L"Software\\Classes\\aTrust";

bool WriteInstallRegistry(const std::wstring &install_dir,
                          const std::wstring &version,
                          const std::wstring &uninstall_exe);

bool RemoveInstallRegistry();

// L4（S4）：卸载回退——仅当 `HKCU\Software\Classes\aTrust\Shell\Open\command` 指向本
// 应用安装目录内时删除该键树（绝不触碰官方客户端的机器级注册）。best-effort：失败
// 只返回 false，不阻断卸载。已知边界：卸载器可能提权运行，HKCU 指向发起提权的账户
// （通常仍是同一用户）；非同账户时残留，可由重装后再卸载或手动删除回收。
bool RemoveUrlSchemeRegistration(const std::wstring &install_dir);

// Read InstallDir from HKCU Software\EXV if present.
std::wstring ReadRegisteredInstallDir();

}  // namespace exv::setup
