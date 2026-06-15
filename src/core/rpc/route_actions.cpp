#include "route_actions.hpp"

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

RouteActions::RouteActions() = default;

RouteActions::RouteActions(std::string config_dir)
    : use_cases_(std::move(config_dir)) {}

void RouteActions::register_handlers(AppRpcDispatcher& dispatcher) {
    dispatcher.register_handler("route.list",
        [this](const RpcRequest& req) { return list(req); });
    dispatcher.register_handler("route.add",
        [this](const RpcRequest& req) { return add(req); });
    dispatcher.register_handler("route.remove",
        [this](const RpcRequest& req) { return remove(req); });
    dispatcher.register_handler("route.enable",
        [this](const RpcRequest& req) { return enable(req); });
    dispatcher.register_handler("route.disable",
        [this](const RpcRequest& req) { return disable(req); });

    // Desktop API names (match webui/desktop/shared/desktop-contract.ts)
    dispatcher.register_handler("routes.list",
        [this](const RpcRequest& req) { return list(req); });
    dispatcher.register_handler("routes.add",
        [this](const RpcRequest& req) { return add(req); });
    dispatcher.register_handler("routes.remove",
        [this](const RpcRequest& req) { return remove(req); });
    dispatcher.register_handler("routes.reset",
        [this](const RpcRequest& req) { return reset(req); });
}

RpcResponse RouteActions::list(const RpcRequest& req) {
    return to_rpc_response(use_cases_.list_routes());
}

RpcResponse RouteActions::add(const RpcRequest& req) {
    try {
        auto payload = json::parse(req.payload_json);
        return to_rpc_response(use_cases_.add_route(payload));
    } catch (const std::exception& e) {
        return invalid_payload_response(e);
    }
}

RpcResponse RouteActions::remove(const RpcRequest& req) {
    try {
        auto payload = json::parse(req.payload_json);
        return to_rpc_response(use_cases_.remove_route(payload));
    } catch (const std::exception& e) {
        return invalid_payload_response(e);
    }
}

RpcResponse RouteActions::enable(const RpcRequest& req) {
    return to_rpc_response(use_cases_.route_enable_unsupported());
}

RpcResponse RouteActions::disable(const RpcRequest& req) {
    return to_rpc_response(use_cases_.route_disable_unsupported());
}

RpcResponse RouteActions::reset(const RpcRequest& req) {
    return to_rpc_response(use_cases_.reset_routes());
}

} // namespace exv::core_api
