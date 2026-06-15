#pragma once
#include "app_rpc_dispatcher.hpp"
#include "core/use_cases/config_use_cases.hpp"
#include <string>

namespace exv::core_api {

class ConfigActions {
public:
    ConfigActions();
    explicit ConfigActions(std::string config_dir);

    void register_handlers(AppRpcDispatcher& dispatcher);

    RpcResponse get(const RpcRequest& req);
    RpcResponse save(const RpcRequest& req);
    RpcResponse get_profile(const RpcRequest& req);
    RpcResponse save_profile(const RpcRequest& req);

private:
    exv::core::ConfigUseCases use_cases_;
};

} // namespace exv::core_api
