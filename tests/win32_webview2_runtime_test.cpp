#include "platform/win32/ui_shell/webview2_runtime_win32.hpp"
#include "platform/win32/ui_shell/webview2_host_win32.hpp"

#include <cassert>
#include <functional>
#include <string>

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

  WebView2BootstrapDecision denied =
      decide_webview2_bootstrap({false, "", ""}, false, false);
  assert(!denied.should_download);
  assert(denied.reason == "offline");

  WebView2BootstrapDecision declined =
      decide_webview2_bootstrap({false, "", ""}, true, false);
  assert(!declined.should_download);
  assert(declined.reason == "user_declined");

  WebView2BootstrapDecision allowed =
      decide_webview2_bootstrap({false, "", ""}, true, true);
  assert(allowed.should_download);
  assert(allowed.reason == "missing");
  assert(allowed.installer_args == "/silent /install");

  WebView2BootstrapDecision unnecessary =
      decide_webview2_bootstrap({true, "120.0.2210.91", "HKCU"}, true, true);
  assert(!unnecessary.should_download);
  assert(unnecessary.reason == "installed");

  bool bridge_invoked = false;
  const std::string bridge_response = dispatch_webview2_host_message(
      R"({"id":9,"action":"status.get","payload":{}})",
      [&](const ecnuvpn::ui_shell::CoreRpcRequest &request) {
        bridge_invoked = true;
        assert(request.action == "status.get");
        ecnuvpn::ui_shell::CoreRpcResponse response;
        response.id = 9;
        response.ok = true;
        response.data_json = R"({"phase":"idle"})";
        return response;
      });
  assert(bridge_invoked);
  assert(bridge_response == R"({"id":9,"ok":true,"data":{"phase":"idle"}})");

  bool post_invoked = false;
  std::string posted_response;
  post_webview2_host_response(
      R"({"id":11,"action":"status.get","payload":{}})",
      [&](const ecnuvpn::ui_shell::CoreRpcRequest &request) {
        post_invoked = true;
        assert(request.action == "status.get");
        ecnuvpn::ui_shell::CoreRpcResponse response;
        response.id = 11;
        response.request_id = request.request_id;
        response.ok = true;
        response.data_json = R"({"phase":"idle"})";
        return response;
      },
      [&](const std::string &response_json) {
        posted_response = response_json;
      });
  assert(post_invoked);
  assert(posted_response == R"({"id":11,"ok":true,"data":{"phase":"idle"}})");

  (void)detect_webview2_runtime();
}
