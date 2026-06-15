#include "platform/win32/ui_shell/webview2_host_win32.hpp"

namespace ecnuvpn::platform::win32::ui_shell {

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

int run_webview2_host(const ecnuvpn::ui_shell::UiWindowConfig &) {
  return 70;
}

} // namespace ecnuvpn::platform::win32::ui_shell
