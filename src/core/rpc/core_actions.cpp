#include "core_actions.hpp"

#include <nlohmann/json.hpp>

#include <exception>
#include <string>
#include <utility>

using json = nlohmann::json;

namespace exv::core_api {
namespace {

RpcResponse invalid_payload_response(const std::exception& e) {
    RpcResponse resp;
    resp.success = false;
    resp.error_code = "invalid_payload";
    resp.error_message = e.what();
    return resp;
}

RpcResponse unsupported_contract_response(const std::string& requested) {
    RpcResponse resp;
    resp.success = false;
    resp.error_code = "unsupported_contract_version";
    resp.error_message = requested.empty()
        ? "The requested contract version is not supported."
        : "Unsupported contract version: " + requested;
    return resp;
}

} // namespace

CoreActions::CoreActions()
    : identity_(exv::core::lifecycle::make_core_identity()) {}

CoreActions::CoreActions(exv::core::lifecycle::CoreIdentity identity)
    : identity_(std::move(identity)) {}

void CoreActions::register_handlers(AppRpcDispatcher& dispatcher) {
    dispatcher.register_handler("core.hello",
        [this](const RpcRequest& req) { return hello(req); });
}

RpcResponse CoreActions::hello(const RpcRequest& req) {
    try {
        json payload = req.payload_json.empty()
            ? json::object()
            : json::parse(req.payload_json);

        if (payload.is_null()) {
            payload = json::object();
        }
        if (!payload.is_object()) {
            throw std::runtime_error("core.hello payload must be a JSON object");
        }

        const std::string requested =
            payload.value("contract_version", std::string());
        if (!exv::core::lifecycle::accepts_contract_version(requested)) {
            return unsupported_contract_response(requested);
        }

        RpcResponse resp;
        resp.success = true;
        resp.payload_json =
            exv::core::lifecycle::core_hello_payload(identity_).dump();
        return resp;
    } catch (const std::exception& e) {
        return invalid_payload_response(e);
    }
}

} // namespace exv::core_api
