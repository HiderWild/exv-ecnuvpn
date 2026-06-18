// Tests for AppRpcDispatcher: handler registration, dispatch, error handling.

#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "core/rpc/rpc_action_metadata.hpp"

#include <iostream>
#include <stdexcept>
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
    using exv::core_api::RpcActionMetadata;
    using exv::core_api::RpcConflictClass;
    using exv::core_api::RpcLane;
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

    // --- duplicate handler registration is rejected ---
    {
        AppRpcDispatcher dispatcher;

        dispatcher.register_handler("vpn.status", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = true;
            resp.payload_json = R"({"version":1})";
            return resp;
        });

        bool duplicate_rejected = false;
        try {
            dispatcher.register_handler("vpn.status", [](const RpcRequest&) {
                RpcResponse resp;
                resp.success = true;
                resp.payload_json = R"({"version":2})";
                return resp;
            });
        } catch (const std::logic_error&) {
            duplicate_rejected = true;
        }

        RpcRequest req;
        req.action = "vpn.status";
        auto resp = dispatcher.dispatch(req);
        ok = expect(duplicate_rejected,
                    "duplicate handler registration should be rejected") && ok;
        ok = expect(resp.payload_json.find("version\":1") != std::string::npos,
                    "original handler should remain registered") && ok;
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

    // --- dispatch preserves legacy array payloads ---
    {
        using exv::core_api::DesktopRpcAdapter;

        DesktopRpcAdapter adapter;
        adapter.register_legacy_handler("logs.list", [](const nlohmann::json&) {
            return nlohmann::json::array({
                { {"level", "info"}, {"message", "first"} },
                { {"level", "error"}, {"message", "second"} },
            });
        });

        const auto result = adapter.dispatch("logs.list", nlohmann::json::object());
        ok = expect(result.is_array(),
                    "legacy array payloads should remain arrays after dispatch") && ok;
        ok = expect(result.size() == 2,
                    "legacy array payload should preserve entry count") && ok;
        ok = expect(result[0].value("message", std::string()) == "first",
                    "legacy array payload should preserve entry contents") && ok;
    }

    // --- explicit action metadata is retained ---
    {
        AppRpcDispatcher dispatcher;
        dispatcher.register_handler(
            "logs.list",
            [](const RpcRequest&) {
                RpcResponse resp;
                resp.success = true;
                resp.payload_json = "[]";
                return resp;
            },
            RpcActionMetadata{RpcLane::Diagnostics, RpcConflictClass::None});

        auto meta = dispatcher.metadata_for("logs.list");
        ok = expect(meta.has_value(), "logs.list metadata should be registered") && ok;
        if (meta.has_value()) {
            ok = expect(meta->lane == RpcLane::Diagnostics,
                        "logs.list lane should be diagnostics") && ok;
            ok = expect(meta->conflict == RpcConflictClass::None,
                        "logs.list conflict should be none") && ok;
        }
        ok = expect(!dispatcher.metadata_for("missing.action").has_value(),
                    "missing action should have no metadata") && ok;
    }

    // --- default action metadata classifies known business lanes ---
    {
        AppRpcDispatcher dispatcher;
        dispatcher.register_handler("vpn.connect", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = true;
            return resp;
        });
        dispatcher.register_handler("config.saveAuth", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = true;
            return resp;
        });
        dispatcher.register_handler("service.install", [](const RpcRequest&) {
            RpcResponse resp;
            resp.success = true;
            return resp;
        });

        auto vpn_meta = dispatcher.metadata_for("vpn.connect");
        auto config_meta = dispatcher.metadata_for("config.saveAuth");
        auto admin_meta = dispatcher.metadata_for("service.install");

        ok = expect(vpn_meta.has_value() &&
                        vpn_meta->lane == RpcLane::VpnControl &&
                        vpn_meta->conflict == RpcConflictClass::VpnWorkflowIntent,
                    "vpn.connect should default to vpn_control intent metadata") && ok;
        ok = expect(config_meta.has_value() &&
                        config_meta->lane == RpcLane::ConfigStore &&
                        config_meta->conflict == RpcConflictClass::ConfigWrite,
                    "config.saveAuth should default to config_store write metadata") && ok;
        ok = expect(admin_meta.has_value() &&
                        admin_meta->lane == RpcLane::PlatformAdmin &&
                        admin_meta->conflict == RpcConflictClass::PlatformAdminWrite,
                    "service.install should default to platform_admin write metadata") && ok;
    }

    // --- desktop adapter exposes metadata for legacy handlers ---
    {
        using exv::core_api::DesktopRpcAdapter;

        DesktopRpcAdapter adapter;
        adapter.register_legacy_handler("logs.list", [](const nlohmann::json&) {
            return nlohmann::json::array();
        });
        auto meta = adapter.dispatcher().metadata_for("logs.list");
        ok = expect(meta.has_value() && meta->lane == RpcLane::Diagnostics,
                    "legacy logs.list handler should keep default diagnostics metadata") && ok;
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
