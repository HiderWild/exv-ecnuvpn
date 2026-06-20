#pragma once

#include <functional>
#include <string>

namespace exv::platform::win32::ui_shell {

struct WebView2RuntimeStatus {
  bool installed = false;
  std::string version;
  std::string source;
};

struct WebView2BootstrapDecision {
  bool should_download = false;
  std::string reason;
  std::string installer_args;
};

bool is_valid_webview2_version(const std::string &version);
WebView2RuntimeStatus evaluate_webview2_runtime_versions(
    const std::string &hklm_version, const std::string &hkcu_version);
WebView2RuntimeStatus detect_webview2_runtime();
WebView2BootstrapDecision decide_webview2_bootstrap(
    const WebView2RuntimeStatus &status, bool network_available,
    bool user_consented);

bool is_allowed_webview2_bootstrapper_url(const std::string &download_url);
using WebView2BootstrapRunner =
    std::function<bool(const std::string &installer_path,
                       const std::string &installer_args)>;

bool run_webview2_evergreen_bootstrapper_with_runner(
    const std::string &download_url, const std::string &installer_path,
    const WebView2BootstrapRunner &runner);
bool run_webview2_evergreen_bootstrapper(const std::string &download_url,
                                         const std::string &installer_path);

} // namespace exv::platform::win32::ui_shell
