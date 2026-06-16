#pragma once

#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/ui_window.hpp"

#include <functional>
#include <memory>
#include <string>

namespace ecnuvpn::platform::win32::ui_shell {

std::string dispatch_webview2_host_message(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core);

void post_webview2_host_response(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core,
    const std::function<void(const std::string &)> &post_response);

std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_webview2_window();

} // namespace ecnuvpn::platform::win32::ui_shell
