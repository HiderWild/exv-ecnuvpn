#pragma once
#include "app_rpc_dispatcher.hpp"
#include "core/use_cases/config_use_cases.hpp"
#include <string>

namespace exv::core_api {

class RouteActions {
public:
    RouteActions();
    explicit RouteActions(std::string config_dir);

    void register_handlers(AppRpcDispatcher& dispatcher);

    RpcResponse list(const RpcRequest& req);
    RpcResponse add(const RpcRequest& req);
    RpcResponse remove(const RpcRequest& req);
    RpcResponse enable(const RpcRequest& req);
    RpcResponse disable(const RpcRequest& req);
    RpcResponse reset(const RpcRequest& req);

private:
    exv::core::ConfigUseCases use_cases_;
};

} // namespace exv::core_api
