#pragma once
#include "app_rpc_dispatcher.hpp"
#include <vector>
#include <string>

namespace exv::core_api {

struct UserRoute {
    std::string destination;
    std::string gateway;
    int metric = 0;
    bool enabled = true;
};

class RouteActions {
public:
    void register_handlers(AppRpcDispatcher& dispatcher);

    RpcResponse list(const RpcRequest& req);
    RpcResponse add(const RpcRequest& req);
    RpcResponse remove(const RpcRequest& req);
    RpcResponse enable(const RpcRequest& req);
    RpcResponse disable(const RpcRequest& req);

private:
    std::vector<UserRoute> user_routes_;
};

} // namespace exv::core_api
