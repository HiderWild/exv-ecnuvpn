#include "route_actions.hpp"
#include <nlohmann/json.hpp>
#include <algorithm>

using json = nlohmann::json;

namespace exv::core_api {

void RouteActions::register_handlers(AppRpcDispatcher& dispatcher) {
    // Legacy names
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
    RpcResponse resp;
    json routes = json::array();
    for (const auto& r : user_routes_) {
        routes.push_back({
            {"destination", r.destination},
            {"gateway", r.gateway},
            {"metric", r.metric},
            {"enabled", r.enabled}
        });
    }
    resp.success = true;
    resp.payload_json = json{{"routes", routes}}.dump();
    return resp;
}

RpcResponse RouteActions::add(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);
        UserRoute route;
        route.destination = payload.at("destination").get<std::string>();
        route.gateway = payload.at("gateway").get<std::string>();
        if (payload.contains("metric")) {
            route.metric = payload["metric"].get<int>();
        }
        if (payload.contains("enabled")) {
            route.enabled = payload["enabled"].get<bool>();
        }
        user_routes_.push_back(std::move(route));
        resp.success = true;
        resp.payload_json = json{{"added", true}}.dump();
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

RpcResponse RouteActions::remove(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);
        std::string dest = payload.at("destination").get<std::string>();

        auto it = std::remove_if(user_routes_.begin(), user_routes_.end(),
            [&dest](const UserRoute& r) { return r.destination == dest; });

        if (it == user_routes_.end()) {
            resp.success = false;
            resp.error_code = "not_found";
            resp.error_message = "No route with destination: " + dest;
        } else {
            user_routes_.erase(it, user_routes_.end());
            resp.success = true;
            resp.payload_json = json{{"removed", true}}.dump();
        }
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

RpcResponse RouteActions::enable(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);
        std::string dest = payload.at("destination").get<std::string>();

        auto it = std::find_if(user_routes_.begin(), user_routes_.end(),
            [&dest](const UserRoute& r) { return r.destination == dest; });

        if (it == user_routes_.end()) {
            resp.success = false;
            resp.error_code = "not_found";
            resp.error_message = "No route with destination: " + dest;
        } else {
            it->enabled = true;
            resp.success = true;
            resp.payload_json = json{{"enabled", true}}.dump();
        }
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

RpcResponse RouteActions::disable(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);
        std::string dest = payload.at("destination").get<std::string>();

        auto it = std::find_if(user_routes_.begin(), user_routes_.end(),
            [&dest](const UserRoute& r) { return r.destination == dest; });

        if (it == user_routes_.end()) {
            resp.success = false;
            resp.error_code = "not_found";
            resp.error_message = "No route with destination: " + dest;
        } else {
            it->enabled = false;
            resp.success = true;
            resp.payload_json = json{{"disabled", true}}.dump();
        }
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

RpcResponse RouteActions::reset(const RpcRequest& req) {
    RpcResponse resp;
    user_routes_.clear();
    resp.success = true;
    resp.payload_json = json{{"reset", true}}.dump();
    return resp;
}

} // namespace exv::core_api
