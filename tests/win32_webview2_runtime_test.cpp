#include "platform/win32/ui_shell/webview2_runtime_win32.hpp"

#include <cassert>

int main() {
  using namespace ecnuvpn::platform::win32::ui_shell;

  assert(is_valid_webview2_version("120.0.2210.91"));
  assert(!is_valid_webview2_version(""));
  assert(!is_valid_webview2_version("0.0.0.0"));
  assert(!is_valid_webview2_version("120.bad"));

  WebView2RuntimeStatus machine =
      evaluate_webview2_runtime_versions("120.0.2210.91", "121.0.0.1");
  assert(machine.installed);
  assert(machine.version == "120.0.2210.91");
  assert(machine.source == "HKLM");

  WebView2RuntimeStatus user =
      evaluate_webview2_runtime_versions("", "121.0.0.1");
  assert(user.installed);
  assert(user.version == "121.0.0.1");
  assert(user.source == "HKCU");

  WebView2RuntimeStatus missing =
      evaluate_webview2_runtime_versions("0.0.0.0", "");
  assert(!missing.installed);

  (void)detect_webview2_runtime();
}
