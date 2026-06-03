#include "config_actions.hpp"
#include <nlohmann/json.hpp>

using json = nlohmann::json;

namespace exv::core_api {

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
    RpcResponse resp;
    // Stub: return empty config
    resp.success = true;
    resp.payload_json = json{{"config", json::object()}}.dump();
    return resp;
}

RpcResponse ConfigActions::save(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);
        // Stub: acknowledge save
        resp.success = true;
        resp.payload_json = json{{"saved", true}}.dump();
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

RpcResponse ConfigActions::get_profile(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);
        std::string profile_id = payload.at("profile_id").get<std::string>();
        // Stub: return empty profile
        resp.success = true;
        resp.payload_json = json{{"profile_id", profile_id}, {"data", json::object()}}.dump();
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

RpcResponse ConfigActions::save_profile(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);
        std::string profile_id = payload.at("profile_id").get<std::string>();
        // Stub: acknowledge save
        resp.success = true;
        resp.payload_json = json{{"profile_id", profile_id}, {"saved", true}}.dump();
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

} // namespace exv::core_api
