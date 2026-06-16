#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/ui_window.hpp"

#include <memory>
#include <string>

namespace ecnuvpn::platform::linux_ui_shell {
std::string dispatch_webkitgtk_host_message(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core);
std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_webkitgtk_window();
}

int main() {
  bool invoked = false;
  bool request_valid = false;
  const std::string response =
      ecnuvpn::platform::linux_ui_shell::dispatch_webkitgtk_host_message(
          R"({"id":9,"action":"status.get","payload":{}})",
          [&](const ecnuvpn::ui_shell::CoreRpcRequest &request) {
            invoked = true;
            request_valid =
                request.request_id == "9" && request.action == "status.get";
            ecnuvpn::ui_shell::CoreRpcResponse out;
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

  auto window = ecnuvpn::platform::linux_ui_shell::create_webkitgtk_window();
  if (!window) {
    return 6;
  }
  window->set_message_handler([](const std::string &) {
    return std::string(R"({"id":1,"ok":true,"data":{}})");
  });
  window->emit_event(R"({"type":"status","data":{}})");
  return 0;
}
