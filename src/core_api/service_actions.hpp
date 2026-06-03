#pragma once
#include "app_rpc_dispatcher.hpp"

namespace exv::core_api {

class ServiceActions {
public:
    void register_handlers(AppRpcDispatcher& dispatcher);

    RpcResponse helper_status(const RpcRequest& req);
    RpcResponse install_helper(const RpcRequest& req);
    RpcResponse uninstall_helper(const RpcRequest& req);
    RpcResponse driver_status(const RpcRequest& req);

private:
    // Platform-specific helpers
};

} // namespace exv::core_api
