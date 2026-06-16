#include "app/ui_shell/ui_window.hpp"
#include "app/ui_shell/host_bridge.hpp"

#include <cassert>
#include <memory>
#include <string>

namespace ecnuvpn::platform::darwin::ui_shell {
std::string dispatch_wkwebview_host_message(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core);
std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_wk_webview_window();
}

int main() {
  bool invoked = false;
  const std::string response =
      ecnuvpn::platform::darwin::ui_shell::dispatch_wkwebview_host_message(
          R"({"id":9,"action":"status.get","payload":{}})",
          [&](const ecnuvpn::ui_shell::CoreRpcRequest &request) {
            invoked = true;
            assert(request.request_id == "9");
            assert(request.action == "status.get");
            ecnuvpn::ui_shell::CoreRpcResponse out;
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

  auto window = ecnuvpn::platform::darwin::ui_shell::create_wk_webview_window();
  if (!window) {
    return 1;
  }
  window->set_message_handler([](const std::string &) {
    return std::string(R"({"id":1,"ok":true,"data":{}})");
  });
  window->emit_event(R"({"type":"status","data":{}})");
  return 0;
}
