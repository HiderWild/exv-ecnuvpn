#include "windows_setup_rust/registry_install.hpp"

#include <algorithm>
#include <cwctype>

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

namespace exv::setup {
namespace {

bool WriteString(HKEY key, const wchar_t *name, const std::wstring &value) {
  return RegSetValueExW(key, name, 0, REG_SZ,
                        reinterpret_cast<const BYTE *>(value.c_str()),
                        static_cast<DWORD>((value.size() + 1) * sizeof(wchar_t))) == ERROR_SUCCESS;
}

bool WriteDword(HKEY key, const wchar_t *name, DWORD value) {
  return RegSetValueExW(key, name, 0, REG_DWORD, reinterpret_cast<const BYTE *>(&value),
                        sizeof(value)) == ERROR_SUCCESS;
}

}  // namespace

bool WriteInstallRegistry(const std::wstring &install_dir,
                          const std::wstring &version,
                          const std::wstring &uninstall_exe) {
  HKEY app = nullptr;
  if (RegCreateKeyExW(HKEY_CURRENT_USER, kAppKey, 0, nullptr, 0, KEY_SET_VALUE, nullptr, &app,
                      nullptr) != ERROR_SUCCESS) {
    return false;
  }
  WriteString(app, L"InstallDir", install_dir);
  RegCloseKey(app);

  HKEY un = nullptr;
  if (RegCreateKeyExW(HKEY_CURRENT_USER, kUninstallKey, 0, nullptr, 0, KEY_SET_VALUE, nullptr, &un,
                      nullptr) != ERROR_SUCCESS) {
    return false;
  }
  const std::wstring icon = install_dir + L"\\exv-ui.exe";
  const std::wstring uninstall_string = L"\"" + uninstall_exe + L"\" /uninstall";

  WriteString(un, L"DisplayName", L"EXV");
  WriteString(un, L"DisplayVersion", version);
  WriteString(un, L"Publisher", L"EXV");
  WriteString(un, L"InstallLocation", install_dir);
  WriteString(un, L"DisplayIcon", icon);
  WriteString(un, L"UninstallString", uninstall_string);
  // S1（2026-10-10 静默移除，R-QUIET 随之关闭）：不再写 QuietUninstallString——
  // 静默卸载已移除，白名单外参数（含 /S）以退出码 3 拒绝，静默串只会指向失效
  // 调用形态。安装时顺带清理旧版本写入的存量值（上方 KEY_SET_VALUE 足以删值；
  // 值不存在时删除失败无害；HKCU 默认视图用 RegDeleteValueW 与写入侧一致——
  // MinGW 头文件不保证声明 Ex 变体）。
  RegDeleteValueW(un, L"QuietUninstallString");
  WriteDword(un, L"NoModify", 1);
  WriteDword(un, L"NoRepair", 1);
  RegCloseKey(un);
  return true;
}

bool RemoveInstallRegistry() {
  RegDeleteTreeW(HKEY_CURRENT_USER, kUninstallKey);
  HKEY app = nullptr;
  if (RegOpenKeyExW(HKEY_CURRENT_USER, kAppKey, 0, KEY_SET_VALUE, &app) == ERROR_SUCCESS) {
    RegDeleteValueW(app, L"InstallDir");
    RegCloseKey(app);
  }
  // Remove key if empty — ignore errors.
  RegDeleteKeyW(HKEY_CURRENT_USER, kAppKey);
  return true;
}

bool RemoveUrlSchemeRegistration(const std::wstring &install_dir) {
  // L4（S4）卸载回退：exv-ui 的 aTrust:// 注册在 HKCU\Software\Classes\aTrust（用户级，
  // 见 tauri/app/src/atrust_scheme.rs）。守卫：只有命令默认值指向**本安装目录**内的 exe
  // 才删——官方客户端的注册在 HKLM（机器级），本函数只动 HKCU 且做路径包含校验，
  // 双保险保证不会误删官方或第三方的注册。
  if (install_dir.empty()) {
    return false;
  }
  HKEY command = nullptr;
  if (RegOpenKeyExW(HKEY_CURRENT_USER,
                    std::wstring(kUrlSchemeKey).append(L"\\Shell\\Open\\command").c_str(), 0,
                    KEY_QUERY_VALUE, &command) != ERROR_SUCCESS) {
    return false;  // 未注册（或键被他人占用为非命令形态）——无事可做。
  }
  wchar_t buffer[1024] = {};
  DWORD size = sizeof(buffer);
  const bool read_ok = RegQueryValueExW(command, nullptr, nullptr, nullptr,
                                        reinterpret_cast<LPBYTE>(buffer), &size) == ERROR_SUCCESS;
  RegCloseKey(command);
  if (!read_ok) {
    return false;
  }
  // 读出的 REG_SZ 按字节长度截取并去掉结尾 NUL，避免依赖缓冲区终止。
  std::wstring claim(buffer, size / sizeof(wchar_t));
  while (!claim.empty() && claim.back() == L'\0') {
    claim.pop_back();
  }
  // 大小写不敏感包含：安装目录是本应用专属前缀（如 %LOCALAPPDATA%\Programs\EXV）。
  std::wstring lower_claim = claim;
  std::wstring lower_dir = install_dir;
  std::transform(lower_claim.begin(), lower_claim.end(), lower_claim.begin(), ::towlower);
  std::transform(lower_dir.begin(), lower_dir.end(), lower_dir.begin(), ::towlower);
  if (lower_claim.find(lower_dir) == std::wstring::npos) {
    return false;  // 认领者不是本安装目录（官方/他址的 EXV 副本）——不动。
  }
  return RegDeleteTreeW(HKEY_CURRENT_USER, kUrlSchemeKey) == ERROR_SUCCESS;
}

std::wstring ReadRegisteredInstallDir() {
  HKEY app = nullptr;
  if (RegOpenKeyExW(HKEY_CURRENT_USER, kAppKey, 0, KEY_QUERY_VALUE, &app) != ERROR_SUCCESS) {
    return {};
  }
  wchar_t buffer[MAX_PATH] = {};
  DWORD type = 0;
  DWORD size = sizeof(buffer);
  const LONG rc =
      RegQueryValueExW(app, L"InstallDir", nullptr, &type, reinterpret_cast<LPBYTE>(buffer), &size);
  RegCloseKey(app);
  if (rc != ERROR_SUCCESS || type != REG_SZ) {
    return {};
  }
  return buffer;
}

}  // namespace exv::setup
