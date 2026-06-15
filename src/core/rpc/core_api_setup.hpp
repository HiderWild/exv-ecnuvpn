#pragma once
#include "app_rpc_dispatcher.hpp"
#include <memory>

namespace exv::core {
class TunnelController;
}

namespace exv::core_api {

// Create and configure the RPC dispatcher with all action handlers
std::unique_ptr<AppRpcDispatcher> create_dispatcher(
    std::shared_ptr<exv::core::TunnelController> controller
);

} // namespace exv::core_api
