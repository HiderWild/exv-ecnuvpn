#pragma once
#include "app_rpc_dispatcher.hpp"
#include <string>

namespace exv::core_api {

class ConfigActions {
public:
    void register_handlers(AppRpcDispatcher& dispatcher);

    RpcResponse get(const RpcRequest& req);
    RpcResponse save(const RpcRequest& req);
    RpcResponse get_profile(const RpcRequest& req);
    RpcResponse save_profile(const RpcRequest& req);

private:
    std::string config_path_;
};

} // namespace exv::core_api
