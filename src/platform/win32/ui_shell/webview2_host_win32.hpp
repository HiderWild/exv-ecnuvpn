#pragma once

#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/ui_window.hpp"

#include <string>

namespace ecnuvpn::platform::win32::ui_shell {

std::string dispatch_webview2_host_message(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core);

int run_webview2_host(const ecnuvpn::ui_shell::UiWindowConfig &config);

} // namespace ecnuvpn::platform::win32::ui_shell
