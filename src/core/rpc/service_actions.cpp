#include "service_actions.hpp"
#include <nlohmann/json.hpp>

using json = nlohmann::json;

namespace exv::core_api {

void ServiceActions::register_handlers(AppRpcDispatcher& dispatcher) {
    // Legacy names
    dispatcher.register_handler("service.helper_status",
        [this](const RpcRequest& req) { return helper_status(req); });
    dispatcher.register_handler("service.install",
        [this](const RpcRequest& req) { return install_helper(req); });
    dispatcher.register_handler("service.uninstall",
        [this](const RpcRequest& req) { return uninstall_helper(req); });
    dispatcher.register_handler("service.driver_status",
        [this](const RpcRequest& req) { return driver_status(req); });

    // Desktop API names (match webui/desktop/shared/desktop-contract.ts)
    dispatcher.register_handler("service.status",
        [this](const RpcRequest& req) { return helper_status(req); });
    dispatcher.register_handler("helper.status",
        [this](const RpcRequest& req) { return helper_status(req); });
    dispatcher.register_handler("drivers.status",
        [this](const RpcRequest& req) { return driver_status(req); });
    dispatcher.register_handler("drivers.install",
        [this](const RpcRequest& req) { return install_driver(req); });
}

RpcResponse ServiceActions::helper_status(const RpcRequest& req) {
    RpcResponse resp;
    // Stub
    resp.success = true;
    resp.payload_json = json{
        {"installed", false},
        {"status", "unknown"},
        {"version", ""}
    }.dump();
    return resp;
}

RpcResponse ServiceActions::install_helper(const RpcRequest& req) {
    RpcResponse resp;
    // Stub
    resp.success = false;
    resp.error_code = "not_implemented";
    resp.error_message = "Helper installation not yet implemented";
    return resp;
}

RpcResponse ServiceActions::uninstall_helper(const RpcRequest& req) {
    RpcResponse resp;
    // Stub
    resp.success = false;
    resp.error_code = "not_implemented";
    resp.error_message = "Helper uninstallation not yet implemented";
    return resp;
}

RpcResponse ServiceActions::driver_status(const RpcRequest& req) {
    RpcResponse resp;
    // Stub
    resp.success = true;
    resp.payload_json = json{
        {"installed", false},
        {"status", "unknown"}
    }.dump();
    return resp;
}

RpcResponse ServiceActions::install_driver(const RpcRequest& req) {
    RpcResponse resp;
    // Stub - driver installation not yet implemented
    resp.success = false;
    resp.error_code = "not_implemented";
    resp.error_message = "Driver installation not yet implemented";
    return resp;
}

} // namespace exv::core_api
