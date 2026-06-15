#pragma once
#include "app_rpc_dispatcher.hpp"
#include "core/use_cases/system_status_use_cases.hpp"

#include <memory>
#include <string>

namespace exv::core {
class TunnelController;
}

namespace exv::core_api {

class ServiceActions {
public:
    ServiceActions();
    explicit ServiceActions(std::shared_ptr<exv::core::TunnelController> controller);
    explicit ServiceActions(std::string config_dir);
    ServiceActions(std::string config_dir,
                   std::shared_ptr<exv::core::TunnelController> controller);

    void register_handlers(AppRpcDispatcher& dispatcher);

    RpcResponse helper_status(const RpcRequest& req);
    RpcResponse install_helper(const RpcRequest& req);
    RpcResponse uninstall_helper(const RpcRequest& req);
    RpcResponse driver_status(const RpcRequest& req);
    RpcResponse install_driver(const RpcRequest& req);

private:
    exv::core::SystemStatusUseCases use_cases_;
    std::shared_ptr<exv::core::TunnelController> controller_;
};

} // namespace exv::core_api
