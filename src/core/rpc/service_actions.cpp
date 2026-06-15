#include "service_actions.hpp"

#include <nlohmann/json.hpp>

#include <exception>
#include <utility>

using json = nlohmann::json;

namespace exv::core_api {
namespace {

RpcResponse to_rpc_response(const exv::core::UseCaseResult &result) {
    RpcResponse resp;
    resp.success = result.success;
    if (result.success) {
        resp.payload_json = result.payload.dump();
    } else {
        resp.error_code = result.error_code;
        resp.error_message = result.error_message;
    }
    return resp;
}

RpcResponse invalid_payload_response(const std::exception &e) {
    return to_rpc_response(
        exv::core::UseCaseResult::fail("invalid_payload", e.what()));
}

} // namespace

ServiceActions::ServiceActions() = default;

ServiceActions::ServiceActions(std::string config_dir)
    : use_cases_(std::move(config_dir)) {}

void ServiceActions::register_handlers(AppRpcDispatcher& dispatcher) {
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
    if (req.action == "service.status") {
        return to_rpc_response(use_cases_.service_status());
    }
    return to_rpc_response(use_cases_.helper_status());
}

RpcResponse ServiceActions::install_helper(const RpcRequest& req) {
    return to_rpc_response(use_cases_.install_helper_unsupported());
}

RpcResponse ServiceActions::uninstall_helper(const RpcRequest& req) {
    return to_rpc_response(use_cases_.uninstall_helper_unsupported());
}

RpcResponse ServiceActions::driver_status(const RpcRequest& req) {
    return to_rpc_response(use_cases_.driver_status());
}

RpcResponse ServiceActions::install_driver(const RpcRequest& req) {
    try {
        auto payload = json::parse(req.payload_json);
        return to_rpc_response(use_cases_.install_driver(payload));
    } catch (const std::exception& e) {
        return invalid_payload_response(e);
    }
}

} // namespace exv::core_api
