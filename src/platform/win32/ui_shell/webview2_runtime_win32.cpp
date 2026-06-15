#include "platform/win32/ui_shell/webview2_runtime_win32.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <algorithm>
#include <cctype>
#include <string>
#include <string_view>

namespace ecnuvpn::platform::win32::ui_shell {
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
      installer_path.empty()) {
    return false;
  }

  // Native download/install execution is intentionally not wired in this slice.
  return false;
}

} // namespace ecnuvpn::platform::win32::ui_shell
