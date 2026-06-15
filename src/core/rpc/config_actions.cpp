#include "config_actions.hpp"

#include <nlohmann/json.hpp>

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

exv::core::UseCaseResult invalid_payload(const std::exception &e) {
    return exv::core::UseCaseResult::fail("invalid_payload", e.what());
}

} // namespace

ConfigActions::ConfigActions() = default;

ConfigActions::ConfigActions(std::string config_dir)
    : use_cases_(std::move(config_dir)) {}

void ConfigActions::register_handlers(AppRpcDispatcher& dispatcher) {
    dispatcher.register_handler("config.get",
        [this](const RpcRequest& req) { return get(req); });
    dispatcher.register_handler("config.save",
        [this](const RpcRequest& req) { return save(req); });

    dispatcher.register_handler("config.get_profile",
        [this](const RpcRequest& req) { return get_profile(req); });
    dispatcher.register_handler("config.save_profile",
        [this](const RpcRequest& req) { return save_profile(req); });
}

RpcResponse ConfigActions::get(const RpcRequest& req) {
    return to_rpc_response(use_cases_.get_config());
}

RpcResponse ConfigActions::save(const RpcRequest& req) {
    try {
        auto payload = json::parse(req.payload_json);
        return to_rpc_response(use_cases_.save_config(payload));
    } catch (const std::exception& e) {
        return to_rpc_response(invalid_payload(e));
    }
}

RpcResponse ConfigActions::get_profile(const RpcRequest& req) {
    try {
        auto payload = json::parse(req.payload_json);
        return to_rpc_response(use_cases_.get_profile(payload));
    } catch (const std::exception& e) {
        return to_rpc_response(invalid_payload(e));
    }
}

RpcResponse ConfigActions::save_profile(const RpcRequest& req) {
    try {
        auto payload = json::parse(req.payload_json);
        return to_rpc_response(use_cases_.save_profile(payload));
    } catch (const std::exception& e) {
        return to_rpc_response(invalid_payload(e));
    }
}

} // namespace exv::core_api
