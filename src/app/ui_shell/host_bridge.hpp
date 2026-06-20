#pragma once

#include "app/ui_shell/core_rpc_client.hpp"

#include <functional>
#include <string>
#include <string_view>

namespace exv::ui_shell {

using CoreRpcInvoker = std::function<CoreRpcResponse(const CoreRpcRequest &)>;

bool is_allowed_host_action(std::string_view action);
std::string handle_host_request(const std::string &request_json,
                                const CoreRpcInvoker &invoke_core);

} // namespace exv::ui_shell
