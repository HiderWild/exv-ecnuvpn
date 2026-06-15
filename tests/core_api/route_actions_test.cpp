// Tests for RouteActions: list, add, remove, enable, disable.

#include "core/rpc/route_actions.hpp"
#include "core/rpc/app_rpc_dispatcher.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <string>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

struct RouteActionsFixture {
    exv::core_api::AppRpcDispatcher dispatcher;
    exv::core_api::RouteActions route;

    RouteActionsFixture() {
        route.register_handlers(dispatcher);
    }

    exv::core_api::RpcResponse dispatch(const std::string& action,
                                         const std::string& payload = "{}") {
        exv::core_api::RpcRequest req;
        req.action = action;
        req.payload_json = payload;
        req.request_id = "test-req";
        return dispatcher.dispatch(req);
    }
};

} // namespace

int main() {
    bool ok = true;

    // --- list returns empty routes initially ---
    {
        RouteActionsFixture fix;
        auto resp = fix.dispatch("route.list");
        ok = expect(resp.success, "route.list should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("routes"), "response missing 'routes'") && ok;
        ok = expect(payload["routes"].is_array(), "routes should be an array") && ok;
        ok = expect(payload["routes"].empty(), "routes should be empty initially") && ok;
    }

    // --- add adds a route ---
    {
        RouteActionsFixture fix;
        auto resp = fix.dispatch("route.add", R"({
            "destination": "10.0.0.0/8",
            "gateway": "192.168.1.1",
            "metric": 100,
            "enabled": true
        })");
        ok = expect(resp.success, "route.add should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload["added"] == true, "added should be true") && ok;

        // Verify the route appears in list
        auto list_resp = fix.dispatch("route.list");
        auto list_payload = json::parse(list_resp.payload_json);
        ok = expect(list_payload["routes"].size() == 1,
                    "routes should have 1 entry after add") && ok;

        auto& r = list_payload["routes"][0];
        ok = expect(r["destination"] == "10.0.0.0/8",
                    "destination should match") && ok;
        ok = expect(r["gateway"] == "192.168.1.1",
                    "gateway should match") && ok;
        ok = expect(r["metric"] == 100, "metric should match") && ok;
        ok = expect(r["enabled"] == true, "enabled should match") && ok;
    }

    // --- add with missing required fields returns error ---
    {
        RouteActionsFixture fix;
        auto resp = fix.dispatch("route.add", R"({"gateway":"10.0.0.1"})");
        ok = expect(!resp.success, "add with missing destination should fail") && ok;
        ok = expect(resp.error_code == "invalid_payload",
                    "error code should be invalid_payload") && ok;
    }

    // --- remove removes a route ---
    {
        RouteActionsFixture fix;
        // Add a route first
        fix.dispatch("route.add", R"({"destination":"172.16.0.0/12","gateway":"10.0.0.1"})");

        auto resp = fix.dispatch("route.remove", R"({"destination":"172.16.0.0/12"})");
        ok = expect(resp.success, "route.remove should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload["removed"] == true, "removed should be true") && ok;

        // Verify the route is gone
        auto list_resp = fix.dispatch("route.list");
        auto list_payload = json::parse(list_resp.payload_json);
        ok = expect(list_payload["routes"].empty(),
                    "routes should be empty after remove") && ok;
    }

    // --- remove nonexistent returns error ---
    {
        RouteActionsFixture fix;
        auto resp = fix.dispatch("route.remove", R"({"destination":"192.168.99.0/24"})");
        ok = expect(!resp.success, "remove nonexistent route should fail") && ok;
        ok = expect(resp.error_code == "not_found",
                    "error code should be 'not_found'") && ok;
        ok = expect(resp.error_message.find("192.168.99.0/24") != std::string::npos,
                    "error message should mention the destination") && ok;
    }

    // --- enable enables a route ---
    {
        RouteActionsFixture fix;
        // Add a disabled route
        fix.dispatch("route.add", R"({
            "destination": "10.1.0.0/16",
            "gateway": "10.0.0.1",
            "enabled": false
        })");

        auto resp = fix.dispatch("route.enable", R"({"destination":"10.1.0.0/16"})");
        ok = expect(resp.success, "route.enable should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload["enabled"] == true, "enabled should be true") && ok;

        // Verify the route is now enabled
        auto list_resp = fix.dispatch("route.list");
        auto list_payload = json::parse(list_resp.payload_json);
        ok = expect(list_payload["routes"][0]["enabled"] == true,
                    "route should be enabled in list") && ok;
    }

    // --- disable disables a route ---
    {
        RouteActionsFixture fix;
        // Add an enabled route
        fix.dispatch("route.add", R"({
            "destination": "10.2.0.0/16",
            "gateway": "10.0.0.1",
            "enabled": true
        })");

        auto resp = fix.dispatch("route.disable", R"({"destination":"10.2.0.0/16"})");
        ok = expect(resp.success, "route.disable should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload["disabled"] == true, "disabled should be true") && ok;

        // Verify the route is now disabled
        auto list_resp = fix.dispatch("route.list");
        auto list_payload = json::parse(list_resp.payload_json);
        ok = expect(list_payload["routes"][0]["enabled"] == false,
                    "route should be disabled in list") && ok;
    }

    // --- enable nonexistent returns error ---
    {
        RouteActionsFixture fix;
        auto resp = fix.dispatch("route.enable", R"({"destination":"no.such.route"})");
        ok = expect(!resp.success, "enable nonexistent route should fail") && ok;
        ok = expect(resp.error_code == "not_found",
                    "error code should be 'not_found'") && ok;
    }

    // --- disable nonexistent returns error ---
    {
        RouteActionsFixture fix;
        auto resp = fix.dispatch("route.disable", R"({"destination":"no.such.route"})");
        ok = expect(!resp.success, "disable nonexistent route should fail") && ok;
        ok = expect(resp.error_code == "not_found",
                    "error code should be 'not_found'") && ok;
    }

    // --- multiple routes ---
    {
        RouteActionsFixture fix;
        fix.dispatch("route.add", R"({"destination":"10.10.0.0/16","gateway":"10.0.0.1"})");
        fix.dispatch("route.add", R"({"destination":"10.20.0.0/16","gateway":"10.0.0.1"})");
        fix.dispatch("route.add", R"({"destination":"10.30.0.0/16","gateway":"10.0.0.1"})");

        auto list_resp = fix.dispatch("route.list");
        auto list_payload = json::parse(list_resp.payload_json);
        ok = expect(list_payload["routes"].size() == 3,
                    "should have 3 routes") && ok;

        // Remove the middle one
        fix.dispatch("route.remove", R"({"destination":"10.20.0.0/16"})");

        list_resp = fix.dispatch("route.list");
        list_payload = json::parse(list_resp.payload_json);
        ok = expect(list_payload["routes"].size() == 2,
                    "should have 2 routes after removal") && ok;
    }

    // --- request_id is propagated ---
    {
        RouteActionsFixture fix;
        exv::core_api::RpcRequest req;
        req.action = "route.list";
        req.payload_json = "{}";
        req.request_id = "trace-route-55";
        auto resp = fix.dispatcher.dispatch(req);
        ok = expect(resp.request_id == "trace-route-55",
                    "request_id should be propagated") && ok;
    }

    if (ok) {
        std::cout << "route_actions_test: all assertions passed\n";
    } else {
        std::cerr << "route_actions_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
