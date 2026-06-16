#include "platform/win32/ui_shell/webview2_host_win32.hpp"

#include <memory>
#include <string>
#include <utility>

namespace ecnuvpn::platform::win32::ui_shell {

namespace {

class WebView2Window final : public ecnuvpn::ui_shell::UiWindow {
public:
  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &) override {
    return 70;
  }

  void emit_event(const std::string &event_json) override {
    last_event_json_ = event_json;
  }

private:
  ecnuvpn::ui_shell::HostMessageHandler handler_;
  std::string last_event_json_;
};

} // namespace

std::string dispatch_webview2_host_message(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core) {
  return ecnuvpn::ui_shell::handle_host_request(message_json, invoke_core);
}

void post_webview2_host_response(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core,
    const std::function<void(const std::string &)> &post_response) {
  post_response(dispatch_webview2_host_message(message_json, invoke_core));
}

std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_webview2_window() {
  return std::make_unique<WebView2Window>();
}

} // namespace ecnuvpn::platform::win32::ui_shell
