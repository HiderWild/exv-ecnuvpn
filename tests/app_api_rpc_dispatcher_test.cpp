// Tests for AppRpcDispatcher: handler registration, dispatch, error handling.

#include "core_api/app_rpc_dispatcher.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

} // namespace

int main() {
    bool ok = true;

    using exv::core_api::AppRpcDispatcher;
    using exv::core_api::RpcRequest;
    using exv::core_api::RpcResponse;

    // --- dispatch to registered handler ---
    {
        AppRpcDispatcher dispatcher;
        dispatcher.register_handler("vpn.status", [](const RpcRequest& req) {
            RpcResponse resp;
            resp.success = true;
            resp.payload_json = R"({"phase":"Connected"})";
            return resp;
        });

        RpcRequest req;
        req.action = "vpn.status";
        req.request_id = "req-1";

        auto resp = dispatcher.dispatch(req);
        ok = expect(resp.success, "dispatch to registered handler should succeed") && ok;
        ok = expect(resp.payload_json.find("Connected") != std::string::npos,
                    "response payload should contain expected data") && ok;
        ok = expect(resp.request_id == "req-1",
                    "response should carry the request_id") && ok;
    }

    // --- dispatch to unregistered handler returns error ---
    {
        AppRpcDispatcher dispatcher;

        RpcRequest req;
        req.action = "vpn.nonexistent";
        req.request_id = "req-2";

        auto resp = dispatcher.dispatch(req);
        ok = expect(!resp.success,
                    "dispatch to unregistered handler should fail") && ok;
        ok = expect(resp.error_code == "unknown_action",
                    "error code should be unknown_action") && ok;
        ok = expect(resp.request_id == "req-2",
                    "error response should carry the request_id") && ok;
    }

    // --- handler receives correct payload ---
    {
        AppRpcDispatcher dispatcher;
        std::string received_payload;
        dispatcher.register_handler("vpn.connect", [&](const RpcRequest& req) {
            received_payload = req.payload_json;
            RpcResponse resp;
            resp.success = true;
            return resp;
        });

        RpcRequest req;
        req.action = "vpn.connect";
        req.payload_json = R"({"server":"vpn.example.edu"})";
        req.request_id = "req-3";

        dispatcher.dispatch(req);
        ok = expect(received_payload.find("vpn.example.edu") != std::string::npos,
                    "handler should receive the payload") && ok;
    }

    // --- multiple handlers registered ---
    {
        AppRpcDispatcher dispatcher;

        dispatcher.register_handler("vpn.status", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = true;
            resp.payload_json = R"({"status":"ok"})";
            return resp;
        });

        dispatcher.register_handler("vpn.disconnect", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = true;
            resp.payload_json = R"({"disconnected":true})";
            return resp;
        });

        RpcRequest status_req;
        status_req.action = "vpn.status";
        auto status_resp = dispatcher.dispatch(status_req);
        ok = expect(status_resp.success, "vpn.status should succeed") && ok;

        RpcRequest disconnect_req;
        disconnect_req.action = "vpn.disconnect";
        auto disconnect_resp = dispatcher.dispatch(disconnect_req);
        ok = expect(disconnect_resp.success, "vpn.disconnect should succeed") && ok;
    }

    // --- handler can return failure ---
    {
        AppRpcDispatcher dispatcher;
        dispatcher.register_handler("vpn.connect", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = false;
            resp.error_code = "auth_failed";
            resp.error_message = "Authentication failed";
            return resp;
        });

        RpcRequest req;
        req.action = "vpn.connect";
        auto resp = dispatcher.dispatch(req);
        ok = expect(!resp.success, "handler can return failure") && ok;
        ok = expect(resp.error_code == "auth_failed",
                    "error code should be preserved") && ok;
    }

    // --- handler overwrite ---
    {
        AppRpcDispatcher dispatcher;

        dispatcher.register_handler("vpn.status", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = true;
            resp.payload_json = R"({"version":1})";
            return resp;
        });

        dispatcher.register_handler("vpn.status", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = true;
            resp.payload_json = R"({"version":2})";
            return resp;
        });

        RpcRequest req;
        req.action = "vpn.status";
        auto resp = dispatcher.dispatch(req);
        ok = expect(resp.payload_json.find("version\":2") != std::string::npos,
                    "overwritten handler should be used") && ok;
    }

    // --- request_id is propagated through handler ---
    {
        AppRpcDispatcher dispatcher;
        std::string handler_received_id;
        dispatcher.register_handler("test.action", [&](const RpcRequest& req) {
            handler_received_id = req.request_id;
            return RpcResponse{true, "{}", "", "", req.request_id};
        });

        RpcRequest req;
        req.action = "test.action";
        req.request_id = "trace-abc-123";
        auto resp = dispatcher.dispatch(req);

        ok = expect(handler_received_id == "trace-abc-123",
                    "handler should receive request_id") && ok;
        ok = expect(resp.request_id == "trace-abc-123",
                    "response should carry request_id from dispatcher") && ok;
    }

    // --- action prefix constants are defined ---
    {
        ok = expect(std::string(AppRpcDispatcher::VPN_PREFIX) == "vpn.",
                    "VPN_PREFIX should be 'vpn.'") && ok;
        ok = expect(std::string(AppRpcDispatcher::CONFIG_PREFIX) == "config.",
                    "CONFIG_PREFIX should be 'config.'") && ok;
        ok = expect(std::string(AppRpcDispatcher::SERVICE_PREFIX) == "service.",
                    "SERVICE_PREFIX should be 'service.'") && ok;
        ok = expect(std::string(AppRpcDispatcher::ROUTE_PREFIX) == "route.",
                    "ROUTE_PREFIX should be 'route.'") && ok;
    }

    if (ok) {
        std::cout << "app_api_rpc_dispatcher_test: all assertions passed\n";
    } else {
        std::cerr << "app_api_rpc_dispatcher_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
