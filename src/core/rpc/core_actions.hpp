#pragma once

#include "app_rpc_dispatcher.hpp"
#include "core/lifecycle/core_identity.hpp"

namespace exv::core_api {

class CoreActions {
public:
    CoreActions();
    explicit CoreActions(exv::core::lifecycle::CoreIdentity identity);

    void register_handlers(AppRpcDispatcher& dispatcher);

    RpcResponse hello(const RpcRequest& req);

private:
    exv::core::lifecycle::CoreIdentity identity_;
};

} // namespace exv::core_api
