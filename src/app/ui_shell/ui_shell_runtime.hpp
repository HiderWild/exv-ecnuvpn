#pragma once

#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/ui_window.hpp"

namespace ecnuvpn::ui_shell {

int run_ui_shell_window(UiWindow &window,
                        const UiWindowConfig &config,
                        CoreRpcClient &client);

} // namespace ecnuvpn::ui_shell
