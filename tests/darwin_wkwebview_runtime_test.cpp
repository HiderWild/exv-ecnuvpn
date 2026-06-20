#include "app/ui_shell/ui_window.hpp"
#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/window_layout.hpp"

#include <cassert>
#include <fstream>
#include <iterator>
#include <memory>
#include <string>
#include <vector>

namespace exv::platform::darwin::ui_shell {
struct WkWebViewStatusMenuItem {
  std::string label;
  int command_id;
  bool separator;
};
std::string dispatch_wkwebview_host_message(
    const std::string &message_json,
    const exv::ui_shell::CoreRpcInvoker &invoke_core);
exv::ui_shell::WindowBounds wkwebview_default_window_bounds() noexcept;
std::vector<WkWebViewStatusMenuItem> wkwebview_status_menu_model();
bool wkwebview_should_create_status_item_on_start();
std::unique_ptr<exv::ui_shell::UiWindow> create_wk_webview_window();
}

int main() {
  const auto default_bounds =
      exv::platform::darwin::ui_shell::wkwebview_default_window_bounds();
  if (default_bounds.width !=
          exv::ui_shell::kElectronAdvancedWindowBounds.width ||
      default_bounds.height !=
          exv::ui_shell::kElectronAdvancedWindowBounds.height) {
    return 1;
  }

  if (!exv::platform::darwin::ui_shell::
          wkwebview_should_create_status_item_on_start()) {
    return 1;
  }
  const auto status_menu =
      exv::platform::darwin::ui_shell::wkwebview_status_menu_model();
  if (status_menu.size() != 3 || status_menu[0].label != "显示 EXV" ||
      !status_menu[1].separator || status_menu[2].label != "退出") {
    return 1;
  }
  const std::string host_source_path =
      std::string(EXV_SOURCE_DIR) +
      "/src/platform/darwin/ui_shell/wk_webview_host_darwin.mm";
  std::ifstream host_source_file(host_source_path);
  const std::string host_source(
      (std::istreambuf_iterator<char>(host_source_file)),
      std::istreambuf_iterator<char>());
  const auto source_contains = [&](const std::string &needle) {
    return host_source.find(needle) != std::string::npos;
  };
  if (!source_contains("if (action == \"window.resizeForMode\")") ||
      !source_contains("if (action == \"window.minimize\")") ||
      !source_contains("if (action == \"window.requestClose\")") ||
      !source_contains("apply_window_mode_once") ||
      !source_contains("NSWindowStyleMaskFullSizeContentView") ||
      !source_contains("setTitlebarAppearsTransparent:YES") ||
      !source_contains("setTitleVisibility:NSWindowTitleHidden") ||
      !source_contains("setOpaque:NO") ||
      !source_contains("drawsBackground")) {
    return 1;
  }

  bool invoked = false;
  const std::string response =
      exv::platform::darwin::ui_shell::dispatch_wkwebview_host_message(
          R"({"id":9,"action":"status.get","payload":{}})",
          [&](const exv::ui_shell::CoreRpcRequest &request) {
            invoked = true;
            assert(request.request_id == "9");
            assert(request.action == "status.get");
            exv::ui_shell::CoreRpcResponse out;
            out.id = 9;
            out.request_id = request.request_id;
            out.ok = true;
            out.data_json = R"({"phase":"idle"})";
            return out;
          });
  assert(invoked);
  assert(response.find(R"("id":9)") != std::string::npos);
  assert(response.find(R"("ok":true)") != std::string::npos);
  assert(response.find(R"("phase":"idle")") != std::string::npos);

  auto window = exv::platform::darwin::ui_shell::create_wk_webview_window();
  if (!window) {
    return 1;
  }
  window->set_message_handler([](const std::string &) {
    return std::string(R"({"id":1,"ok":true,"data":{}})");
  });
  window->emit_event(R"({"type":"status","data":{}})");
  return 0;
}
