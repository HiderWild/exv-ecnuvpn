#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/ui_window.hpp"
#include "app/ui_shell/window_layout.hpp"

#include <memory>
#include <string>

namespace exv::platform::linux_ui_shell {
std::string dispatch_webkitgtk_host_message(
    const std::string &message_json,
    const exv::ui_shell::CoreRpcInvoker &invoke_core);
exv::ui_shell::WindowBounds webkitgtk_default_window_bounds() noexcept;
std::unique_ptr<exv::ui_shell::UiWindow> create_webkitgtk_window();
}

int main() {
  const auto default_bounds =
      exv::platform::linux_ui_shell::webkitgtk_default_window_bounds();
  if (default_bounds.width !=
          exv::ui_shell::kElectronAdvancedWindowBounds.width ||
      default_bounds.height !=
          exv::ui_shell::kElectronAdvancedWindowBounds.height) {
    return 7;
  }

  bool invoked = false;
  bool request_valid = false;
  const std::string response =
      exv::platform::linux_ui_shell::dispatch_webkitgtk_host_message(
          R"({"id":9,"action":"status.get","payload":{}})",
          [&](const exv::ui_shell::CoreRpcRequest &request) {
            invoked = true;
            request_valid =
                request.request_id == "9" && request.action == "status.get";
            exv::ui_shell::CoreRpcResponse out;
            out.id = 9;
            out.request_id = request.request_id;
            out.ok = true;
            out.data_json = R"({"phase":"idle"})";
            return out;
          });
  if (!invoked) {
    return 1;
  }
  if (!request_valid) {
    return 2;
  }
  if (response.find(R"("id":9)") == std::string::npos) {
    return 3;
  }
  if (response.find(R"("ok":true)") == std::string::npos) {
    return 4;
  }
  if (response.find(R"("phase":"idle")") == std::string::npos) {
    return 5;
  }

  auto window = exv::platform::linux_ui_shell::create_webkitgtk_window();
  if (!window) {
    return 6;
  }
  window->set_message_handler([](const std::string &) {
    return std::string(R"({"id":1,"ok":true,"data":{}})");
  });
  window->emit_event(R"({"type":"status","data":{}})");
  return 0;
}
