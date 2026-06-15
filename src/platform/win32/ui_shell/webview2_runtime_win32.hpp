#pragma once

#include <string>

namespace ecnuvpn::platform::win32::ui_shell {

struct WebView2RuntimeStatus {
  bool installed = false;
  std::string version;
  std::string source;
};

bool is_valid_webview2_version(const std::string &version);
WebView2RuntimeStatus evaluate_webview2_runtime_versions(
    const std::string &hklm_version, const std::string &hkcu_version);
WebView2RuntimeStatus detect_webview2_runtime();

} // namespace ecnuvpn::platform::win32::ui_shell
