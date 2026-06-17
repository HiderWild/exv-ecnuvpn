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

  auto window = create_webview2_window();
  assert(window != nullptr);

  const ecnuvpn::ui_shell::RendererAssets packaged_renderer{
      ecnuvpn::ui_shell::RendererAssetKind::PackagedFile,
      "C:/Program Files/ECNU VPN/webui/index.html"};
  const std::wstring packaged_uri = webview2_renderer_uri(packaged_renderer);
  if (packaged_uri != L"https://appassets.ecnu-vpn.invalid/index.html") {
    return 1;
  }
  const std::wstring packaged_folder =
      webview2_packaged_renderer_folder(packaged_renderer);
  if (packaged_folder.find(L"ECNU VPN") == std::wstring::npos ||
      packaged_folder.find(L"webui") == std::wstring::npos ||
      packaged_folder.find(L"index.html") != std::wstring::npos) {
    return 1;
  }
  const std::wstring dev_server_uri = webview2_renderer_uri(
      {ecnuvpn::ui_shell::RendererAssetKind::DevServer,
       "http://127.0.0.1:5173/"});
  if (dev_server_uri != L"http://127.0.0.1:5173/") {
    return 1;
  }

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

  const bool allows_official_bootstrapper_url =
      is_allowed_webview2_bootstrapper_url(
          "https://go.microsoft.com/fwlink/?linkid=2124703");
  const bool allows_case_insensitive_official_bootstrapper_url =
      is_allowed_webview2_bootstrapper_url(
          "HTTPS://GO.MICROSOFT.COM/fwlink/?linkid=2124703");
  const bool allows_documented_official_bootstrapper_url =
      is_allowed_webview2_bootstrapper_url(
          "https://go.microsoft.com/fwlink/p/?LinkId=2124703");
  const bool rejects_arbitrary_bootstrapper_url =
      !is_allowed_webview2_bootstrapper_url(
          "https://example.com/MicrosoftEdgeWebview2Setup.exe");
  const bool rejects_non_https_bootstrapper_url =
      !is_allowed_webview2_bootstrapper_url(
          "http://go.microsoft.com/fwlink/?linkid=2124703");
  const bool rejects_wrong_linkid_bootstrapper_url =
      !is_allowed_webview2_bootstrapper_url(
          "https://go.microsoft.com/fwlink/?linkid=9999999");
  const bool rejects_empty_bootstrapper_url =
      !is_allowed_webview2_bootstrapper_url("");
  if (!allows_official_bootstrapper_url ||
      !allows_case_insensitive_official_bootstrapper_url ||
      !allows_documented_official_bootstrapper_url ||
      !rejects_arbitrary_bootstrapper_url || !rejects_non_https_bootstrapper_url ||
      !rejects_wrong_linkid_bootstrapper_url || !rejects_empty_bootstrapper_url) {
    return 1;
  }

  const bool rejects_disallowed_bootstrapper_run =
      !run_webview2_evergreen_bootstrapper(
          "https://example.com/MicrosoftEdgeWebview2Setup.exe",
          "C:/temp/setup.exe");
  const bool rejects_empty_bootstrapper_installer_path =
      !run_webview2_evergreen_bootstrapper(
          "https://go.microsoft.com/fwlink/?linkid=2124703", "");
  if (!rejects_disallowed_bootstrapper_run ||
      !rejects_empty_bootstrapper_installer_path) {
    return 1;
  }

  bool bootstrap_runner_invoked = false;
  const bool bootstrap_runner_ran =
      run_webview2_evergreen_bootstrapper_with_runner(
          "https://go.microsoft.com/fwlink/?linkid=2124703",
          "C:/temp/MicrosoftEdgeWebview2Setup.exe",
          [&](const std::string &installer_path,
              const std::string &installer_args) {
            bootstrap_runner_invoked = true;
            assert(installer_path ==
                   "C:/temp/MicrosoftEdgeWebview2Setup.exe");
            assert(installer_args == "/silent /install");
            return true;
          });
  assert(bootstrap_runner_ran);
  assert(bootstrap_runner_invoked);

  bool rejected_runner_invoked = false;
  const bool rejected_runner_ran =
      run_webview2_evergreen_bootstrapper_with_runner(
          "https://example.com/MicrosoftEdgeWebview2Setup.exe",
          "C:/temp/MicrosoftEdgeWebview2Setup.exe",
          [&](const std::string &, const std::string &) {
            rejected_runner_invoked = true;
            return true;
          });
  assert(!rejected_runner_ran);
  assert(!rejected_runner_invoked);

  (void)detect_webview2_runtime();
}
