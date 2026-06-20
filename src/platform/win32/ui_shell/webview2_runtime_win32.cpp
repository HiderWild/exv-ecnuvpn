#include "platform/win32/ui_shell/webview2_runtime_win32.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <urlmon.h>
#include <windows.h>

#include <algorithm>
#include <cctype>
#include <string>
#include <string_view>

namespace exv::platform::win32::ui_shell {
namespace {

constexpr wchar_t kWebView2MachineClientKey[] =
    L"SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\"
    L"{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
constexpr wchar_t kWebView2UserClientKey[] =
    L"Software\\Microsoft\\EdgeUpdate\\Clients\\"
    L"{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

std::string utf8_from_wide(const wchar_t *value) {
  if (!value || *value == L'\0') {
    return {};
  }

  const int required =
      WideCharToMultiByte(CP_UTF8, 0, value, -1, nullptr, 0, nullptr, nullptr);
  if (required <= 1) {
    return {};
  }

  std::string out(static_cast<std::size_t>(required), '\0');
  WideCharToMultiByte(CP_UTF8, 0, value, -1, out.data(), required, nullptr,
                      nullptr);
  if (!out.empty() && out.back() == '\0') {
    out.pop_back();
  }
  return out;
}

std::string read_registry_string(HKEY root, const wchar_t *subkey,
                                 const wchar_t *name) {
  wchar_t value[256] = {};
  DWORD value_size = sizeof(value);
  const LSTATUS status =
      RegGetValueW(root, subkey, name, RRF_RT_REG_SZ, nullptr, value,
                   &value_size);
  if (status != ERROR_SUCCESS) {
    return {};
  }
  return utf8_from_wide(value);
}

std::string lowercase_ascii(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char ch) {
    if (ch >= 'A' && ch <= 'Z') {
      return static_cast<char>(ch - 'A' + 'a');
    }
    return static_cast<char>(ch);
  });
  return value;
}

std::wstring wide_from_utf8(const std::string &value) {
  if (value.empty()) {
    return {};
  }
  const int required =
      MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
                          static_cast<int>(value.size()), nullptr, 0);
  if (required <= 0) {
    return {};
  }
  std::wstring out(static_cast<std::size_t>(required), L'\0');
  MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
                      static_cast<int>(value.size()), out.data(), required);
  return out;
}

std::wstring quote_windows_argument(const std::string &value) {
  if (value.empty()) {
    return L"\"\"";
  }
  const bool needs_quotes =
      value.find_first_of(" \t\n\v\"") != std::string::npos;
  std::wstring wide = wide_from_utf8(value);
  if (!needs_quotes) {
    return wide;
  }

  std::wstring quoted;
  quoted.push_back(L'"');
  std::size_t backslashes = 0;
  for (wchar_t ch : wide) {
    if (ch == L'\\') {
      ++backslashes;
      continue;
    }
    if (ch == L'"') {
      quoted.append(backslashes * 2 + 1, L'\\');
      quoted.push_back(ch);
      backslashes = 0;
      continue;
    }
    quoted.append(backslashes, L'\\');
    backslashes = 0;
    quoted.push_back(ch);
  }
  quoted.append(backslashes * 2, L'\\');
  quoted.push_back(L'"');
  return quoted;
}

bool download_webview2_bootstrapper(const std::string &download_url,
                                    const std::string &installer_path) {
  const std::wstring wide_url = wide_from_utf8(download_url);
  const std::wstring wide_path = wide_from_utf8(installer_path);
  if (wide_url.empty() || wide_path.empty()) {
    return false;
  }
  return SUCCEEDED(URLDownloadToFileW(nullptr, wide_url.c_str(),
                                      wide_path.c_str(), 0, nullptr));
}

bool run_installer_process(const std::string &installer_path,
                           const std::string &installer_args) {
  const std::wstring app_path = wide_from_utf8(installer_path);
  if (app_path.empty()) {
    return false;
  }

  std::wstring command_line = quote_windows_argument(installer_path);
  if (!installer_args.empty()) {
    command_line.push_back(L' ');
    command_line.append(wide_from_utf8(installer_args));
  }

  STARTUPINFOW startup_info{};
  startup_info.cb = sizeof(startup_info);
  PROCESS_INFORMATION process_info{};
  const BOOL created = CreateProcessW(
      app_path.c_str(), command_line.data(), nullptr, nullptr, FALSE,
      CREATE_NO_WINDOW, nullptr, nullptr, &startup_info, &process_info);
  if (!created) {
    return false;
  }

  WaitForSingleObject(process_info.hProcess, INFINITE);
  DWORD exit_code = 1;
  GetExitCodeProcess(process_info.hProcess, &exit_code);
  CloseHandle(process_info.hThread);
  CloseHandle(process_info.hProcess);
  return exit_code == 0;
}

} // namespace

bool is_valid_webview2_version(const std::string &version) {
  if (version.empty() || version == "0.0.0.0") {
    return false;
  }

  bool digit = false;
  for (const char ch : version) {
    if (std::isdigit(static_cast<unsigned char>(ch))) {
      digit = true;
      continue;
    }
    if (ch != '.') {
      return false;
    }
  }
  return digit;
}

WebView2RuntimeStatus evaluate_webview2_runtime_versions(
    const std::string &hklm_version, const std::string &hkcu_version) {
  if (is_valid_webview2_version(hklm_version)) {
    return {true, hklm_version, "HKLM"};
  }
  if (is_valid_webview2_version(hkcu_version)) {
    return {true, hkcu_version, "HKCU"};
  }
  return {};
}

WebView2RuntimeStatus detect_webview2_runtime() {
  const std::string hklm =
      read_registry_string(HKEY_LOCAL_MACHINE, kWebView2MachineClientKey, L"pv");
  const std::string hkcu =
      read_registry_string(HKEY_CURRENT_USER, kWebView2UserClientKey, L"pv");
  return evaluate_webview2_runtime_versions(hklm, hkcu);
}

WebView2BootstrapDecision decide_webview2_bootstrap(
    const WebView2RuntimeStatus &status, bool network_available,
    bool user_consented) {
  if (status.installed) {
    return {false, "installed", ""};
  }
  if (!network_available) {
    return {false, "offline", ""};
  }
  if (!user_consented) {
    return {false, "user_declined", ""};
  }
  return {true, "missing", "/silent /install"};
}

bool is_allowed_webview2_bootstrapper_url(const std::string &download_url) {
  constexpr std::string_view kAllowedPrefix = "https://go.microsoft.com";
  constexpr std::string_view kAllowedPathAndQuery =
      "/fwlink/?linkid=2124703";
  constexpr std::string_view kAllowedDocumentedPathAndQuery =
      "/fwlink/p/?linkid=2124703";

  const std::string normalized = lowercase_ascii(download_url);
  if (normalized.rfind(kAllowedPrefix, 0) != 0) {
    return false;
  }

  const std::string_view path_and_query(
      normalized.data() + kAllowedPrefix.size(),
      normalized.size() - kAllowedPrefix.size());
  return path_and_query == kAllowedPathAndQuery ||
         path_and_query == kAllowedDocumentedPathAndQuery;
}

bool run_webview2_evergreen_bootstrapper(const std::string &download_url,
                                         const std::string &installer_path) {
  if (!is_allowed_webview2_bootstrapper_url(download_url) ||
      installer_path.empty() ||
      !download_webview2_bootstrapper(download_url, installer_path)) {
    return false;
  }
  return run_webview2_evergreen_bootstrapper_with_runner(
      download_url, installer_path, run_installer_process);
}

bool run_webview2_evergreen_bootstrapper_with_runner(
    const std::string &download_url, const std::string &installer_path,
    const WebView2BootstrapRunner &runner) {
  if (!is_allowed_webview2_bootstrapper_url(download_url) ||
      installer_path.empty() || !runner) {
    return false;
  }
  return runner(installer_path, "/silent /install");
}

} // namespace exv::platform::win32::ui_shell
