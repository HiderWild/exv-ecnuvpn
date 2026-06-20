#include "platform/win32/ui_shell/webview2_runtime_win32.hpp"
#include "platform/win32/ui_shell/webview2_host_win32.hpp"
#include "app/ui_shell/window_layout.hpp"

#include <algorithm>
#include <cassert>
#include <fstream>
#include <functional>
#include <iterator>
#include <string>
#include <vector>

int main() {
  using namespace exv::platform::win32::ui_shell;
  const auto default_bounds = webview2_default_window_bounds();
  if (default_bounds.width !=
          exv::ui_shell::kElectronAdvancedWindowBounds.width ||
      default_bounds.height !=
          exv::ui_shell::kElectronAdvancedWindowBounds.height) {
    return 1;
  }
  const auto scaled_advanced =
      webview2_window_mode_bounds_for_dpi("advanced", 120);
  if (scaled_advanced.width != 1215 || scaled_advanced.height != 704) {
    return 1;
  }
  const auto scaled_minimal =
      webview2_window_mode_bounds_for_dpi("minimal", 120);
  if (scaled_minimal.width != 378 || scaled_minimal.height != 148) {
    return 1;
  }
  const auto invalid_dpi_bounds =
      webview2_window_mode_bounds_for_dpi("advanced", 0);
  if (invalid_dpi_bounds.width !=
          exv::ui_shell::kElectronAdvancedWindowBounds.width ||
      invalid_dpi_bounds.height !=
          exv::ui_shell::kElectronAdvancedWindowBounds.height) {
    return 1;
  }

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

  if (!webview2_should_create_tray_on_start()) {
    return 1;
  }
  const auto tray_menu = webview2_tray_menu_model();
  if (tray_menu.size() != 3 || tray_menu[0].label != L"显示 EXV" ||
      !tray_menu[1].separator || tray_menu[2].label != L"退出") {
    return 1;
  }
  if (webview2_taskbar_created_message_name() != L"TaskbarCreated") {
    return 1;
  }
  if (webview2_app_icon_resource_id() <= 0 ||
      webview2_app_icon_resource_id() == 32512) {
    return 1;
  }
  const std::string host_source_path =
      std::string(EXV_SOURCE_DIR) +
      "/src/platform/win32/ui_shell/webview2_host_win32.cpp";
  std::ifstream host_source_file(host_source_path);
  const std::string host_source(
      (std::istreambuf_iterator<char>(host_source_file)),
      std::istreambuf_iterator<char>());
  std::string normalized_host_source = host_source;
  normalized_host_source.erase(std::remove(normalized_host_source.begin(),
                                           normalized_host_source.end(), '\r'),
                               normalized_host_source.end());
  const auto source_contains = [&](const std::string &needle) {
    return normalized_host_source.find(needle) != std::string::npos;
  };
  const std::string native_control_hit_test =
      "const LRESULT control_hit =\n"
      "          control_button_hit_test(content_x, content_y, content_width, "
      "dpi);\n"
      "      if (control_hit != HTCLIENT) {\n"
      "        return control_hit;\n"
      "      }";
  if (!source_contains("apply_window_mode_once") ||
      !source_contains("if (action == \"window.resizeForMode\")") ||
      !source_contains("if (action == \"window.minimize\")") ||
      !source_contains("if (action == \"window.requestClose\")") ||
      !source_contains("if (action == \"window.startDrag\")") ||
      !source_contains("GetCursorPos(&cursor)") ||
      !source_contains("renderer_client_to_screen(renderer_start)") ||
      !source_contains(
          "renderer_derived_start.value_or(renderer_start_point.value_or("
          "cursor))") ||
      !source_contains("renderer_titlebar_hit_test(renderer_start)") ||
      !source_contains("left_mouse_button_down()") ||
      !source_contains("start-drag-reject-button-up") ||
      !source_contains("GetCursorPos(&move_loop_start)") ||
      !source_contains("start-drag-current-cursor") ||
      !source_contains("ReleaseCapture()") ||
      !source_contains("WM_NCLBUTTONDOWN") ||
      !source_contains("MAKELPARAM(move_loop_start.x, move_loop_start.y)") ||
      !source_contains("case WM_MOUSEACTIVATE:") ||
      !source_contains("return MA_ACTIVATE;") ||
      !source_contains("WM_NCCALCSIZE") ||
      !source_contains("WM_NCHITTEST") ||
      !source_contains("HTCAPTION") ||
      !source_contains("content_x = x - shadow_margin") ||
      !source_contains("content_y = y - shadow_margin") ||
      !source_contains("return HTNOWHERE;") ||
      !source_contains(native_control_hit_test) ||
      !source_contains("GWL_STYLE") ||
      !source_contains("~WS_CAPTION") ||
      !source_contains("SWP_FRAMECHANGED") ||
      !source_contains("restore_or_focus_window()") ||
      !source_contains("IsIconic(hwnd_)") ||
      !source_contains("GetForegroundWindow() != hwnd_") ||
      !source_contains("configure_non_client_region_support()") ||
      !source_contains("ICoreWebView2Settings9") ||
      !source_contains("put_IsNonClientRegionSupportEnabled(TRUE)") ||
      !source_contains("control_button_hit_test(content_x, content_y, "
                       "content_width, dpi)") ||
      !source_contains("return HTMINBUTTON;") ||
      !source_contains("return HTCLOSE;") ||
      !source_contains("emit_window_control_state(") ||
      !source_contains("event[\"type\"] = \"window-control-state\"") ||
      !source_contains("case WM_NCLBUTTONDOWN:") ||
      !source_contains("case WM_NCLBUTTONUP:") ||
      !source_contains("case WM_NCMOUSELEAVE:") ||
      !source_contains("CreateRoundRectRgn") ||
      !source_contains("SetWindowRgn") ||
      source_contains("GetMessagePos()") ||
      source_contains("SetCapture(hwnd_)") ||
      source_contains("void update_window_drag()") ||
      source_contains("kWindowModeAnimationTimer") ||
      source_contains("SetTimer(hwnd_, kWindowModeAnimationTimer")) {
    return 1;
  }

  const exv::ui_shell::RendererAssets packaged_renderer{
      exv::ui_shell::RendererAssetKind::PackagedFile,
      "C:/Program Files/EXV/webui/index.html"};
  const std::wstring packaged_uri = webview2_renderer_uri(packaged_renderer);
  if (packaged_uri != L"https://appassets.exv.invalid/index.html") {
    return 1;
  }
  const std::wstring packaged_folder =
      webview2_packaged_renderer_folder(packaged_renderer);
  if (packaged_folder.find(L"EXV") == std::wstring::npos ||
      packaged_folder.find(L"webui") == std::wstring::npos ||
      packaged_folder.find(L"index.html") != std::wstring::npos) {
    return 1;
  }
  const std::wstring dev_server_uri = webview2_renderer_uri(
      {exv::ui_shell::RendererAssetKind::DevServer,
       "http://127.0.0.1:5173/"});
  if (dev_server_uri != L"http://127.0.0.1:5173/") {
    return 1;
  }

  bool bridge_invoked = false;
  const std::string bridge_response = dispatch_webview2_host_message(
      R"({"id":9,"action":"status.get","payload":{}})",
      [&](const exv::ui_shell::CoreRpcRequest &request) {
        bridge_invoked = true;
        assert(request.action == "status.get");
        exv::ui_shell::CoreRpcResponse response;
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
      [&](const exv::ui_shell::CoreRpcRequest &request) {
        post_invoked = true;
        assert(request.action == "status.get");
        exv::ui_shell::CoreRpcResponse response;
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
