#pragma once
#include "app_rpc_dispatcher.hpp"
#include <memory>

namespace exv::core {

// Forward declarations
class TunnelController;
struct UserIntent;

} // namespace exv::core

namespace exv::core_api {

class VpnActions {
public:
    explicit VpnActions(std::shared_ptr<exv::core::TunnelController> controller);

    // Register all vpn.* handlers with dispatcher
    void register_handlers(AppRpcDispatcher& dispatcher);

    // Individual actions
    RpcResponse connect(const RpcRequest& req);
    RpcResponse disconnect(const RpcRequest& req);
    RpcResponse status(const RpcRequest& req);
    RpcResponse set_auto_reconnect(const RpcRequest& req);
    RpcResponse get_legacy_status(const RpcRequest& req);

private:
    std::shared_ptr<exv::core::TunnelController> controller_;
};

} // namespace exv::core_api
