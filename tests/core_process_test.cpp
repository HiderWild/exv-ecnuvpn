// Tests for the core process mode: verifies that the RPC dispatcher created
// for `exv --mode=core` correctly handles status.get and other core-exclusive
// actions, and that status event callbacks produce valid JSON.

#include "core/tunnel_controller.hpp"
#include "core/tunnel_state.hpp"
#include "core/tunnel_intent.hpp"
#include "core/reconnect_policy.hpp"
#include "core_api/app_rpc_dispatcher.hpp"
#include "core_api/core_api_setup.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <memory>
#include <string>

#ifndef ECNUVPN_VERSION
#define ECNUVPN_VERSION "test"
#endif

using json = nlohmann::json;
using exv::core::TunnelController;
using exv::core::TunnelPhase;
using exv::core::TunnelStatusSnapshot;
using exv::core::ReconnectConfig;
using exv::core_api::AppRpcDispatcher;
using exv::core_api::RpcRequest;
using exv::core_api::RpcResponse;

static bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

// ---------------------------------------------------------------------------
// Helper: register the same core-exclusive actions that core_process.cpp adds
// ---------------------------------------------------------------------------

static const char* phase_to_string(TunnelPhase p) {
    switch (p) {
        case TunnelPhase::Idle:                return "idle";
        case TunnelPhase::PreparingHelper:     return "preparing_helper";
        case TunnelPhase::Authenticating:      return "authenticating";
        case TunnelPhase::ConnectingCstp:      return "connecting_cstp";
        case TunnelPhase::ApplyingNetworkConfig: return "applying_network_config";
        case TunnelPhase::OpeningPacketDevice: return "opening_packet_device";
        case TunnelPhase::Connected:           return "connected";
        case TunnelPhase::Reconnecting:        return "reconnecting";
        case TunnelPhase::Disconnecting:       return "disconnecting";
        case TunnelPhase::CleaningUp:          return "cleaning_up";
        case TunnelPhase::Failed:              return "failed";
        default:                               return "unknown";
    }
}

static json snapshot_to_json(const TunnelStatusSnapshot& s) {
    json result = {
        {"phase",           phase_to_string(s.phase)},
        {"desired_connected", s.desired_connected},
        {"auto_reconnect",  s.auto_reconnect},
        {"helper_mode",     s.helper_mode},
        {"helper_status",   s.helper_status},
        {"network_ready",   s.network_ready},
        {"server",          s.server},
        {"interface_name",  s.interface_name}
    };
    if (s.last_error.has_value()) {
        auto& err = s.last_error.value();
        result["last_error"] = {
            {"domain",             err.domain},
            {"code",               err.code},
            {"message",            err.message},
            {"recoverable",        err.recoverable},
            {"recommended_action", err.recommended_action}
        };
    }
    if (s.reconnect.has_value()) {
        result["reconnect"] = {
            {"attempt",       s.reconnect->attempt},
            {"next_retry_ms", s.reconnect->next_retry_ms}
        };
    }
    return result;
}

static void register_core_exclusive_actions(
    AppRpcDispatcher& dispatcher,
    std::shared_ptr<TunnelController> controller)
{
    dispatcher.register_handler("status.get",
        [controller](const RpcRequest&) -> RpcResponse {
            RpcResponse resp;
            auto snap = controller->status();
            resp.success = true;
            resp.payload_json = snapshot_to_json(snap).dump();
            return resp;
        });

    dispatcher.register_handler("runtime.status",
        [](const RpcRequest&) -> RpcResponse {
            RpcResponse resp;
            json info;
            info["version"] = ECNUVPN_VERSION;
            info["bootstrapped"] = false; // not bootstrapped in test
            resp.success = true;
            resp.payload_json = info.dump();
            return resp;
        });

    dispatcher.register_handler("service.status",
        [&dispatcher](const RpcRequest& req) -> RpcResponse {
            RpcRequest aliased = req;
            aliased.action = "service.helper_status";
            return dispatcher.dispatch(aliased);
        });

    dispatcher.register_handler("drivers.status",
        [&dispatcher](const RpcRequest& req) -> RpcResponse {
            RpcRequest aliased = req;
            aliased.action = "service.driver_status";
            return dispatcher.dispatch(aliased);
        });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

int main() {
    bool ok = true;

    // Create TunnelController with null stubs (same as core process does)
    auto helper  = std::shared_ptr<exv::helper::HelperClient>(nullptr);
    auto net_ops = std::shared_ptr<exv::platform::PlatformNetworkOps>(nullptr);
    auto controller = std::make_shared<TunnelController>(
        helper, net_ops, ReconnectConfig{});

    // Create dispatcher using the same factory as core_api_setup
    auto dispatcher = exv::core_api::create_dispatcher(controller);

    // Register core-exclusive actions
    register_core_exclusive_actions(*dispatcher, controller);

    // ---- Test 1: status.get returns idle status ----
    {
        RpcRequest req;
        req.action = "status.get";
        req.request_id = "1";

        auto resp = dispatcher->dispatch(req);
        ok = expect(resp.success, "status.get should succeed") && ok;

        auto data = json::parse(resp.payload_json);
        ok = expect(data.value("phase", "") == "idle",
                    "initial phase should be idle") && ok;
        ok = expect(data.value("desired_connected", true) == false,
                    "desired_connected should be false initially") && ok;
        ok = expect(data.value("auto_reconnect", false) == true,
                    "auto_reconnect should be true initially") && ok;
    }

    // ---- Test 2: status.get reflects auto_reconnect change ----
    {
        // Change auto_reconnect and verify status reflects it
        RpcRequest set_req;
        set_req.action = "vpn.set_auto_reconnect";
        set_req.payload_json = R"({"enabled":false})";
        set_req.request_id = "2";

        auto set_resp = dispatcher->dispatch(set_req);
        ok = expect(set_resp.success, "vpn.set_auto_reconnect should succeed") && ok;

        RpcRequest status_req;
        status_req.action = "status.get";
        status_req.request_id = "3";

        auto status_resp = dispatcher->dispatch(status_req);
        ok = expect(status_resp.success, "status.get after set_auto_reconnect should succeed") && ok;

        auto data = json::parse(status_resp.payload_json);
        ok = expect(data.value("auto_reconnect", true) == false,
                    "auto_reconnect should be false after set") && ok;
        ok = expect(data.value("phase", "") == "idle",
                    "phase should still be idle") && ok;
    }

    // ---- Test 3: runtime.status returns version info ----
    {
        RpcRequest req;
        req.action = "runtime.status";
        req.request_id = "4";

        auto resp = dispatcher->dispatch(req);
        ok = expect(resp.success, "runtime.status should succeed") && ok;

        auto data = json::parse(resp.payload_json);
        ok = expect(data.contains("version"), "runtime.status should contain version") && ok;
        ok = expect(data.value("version", "") == ECNUVPN_VERSION,
                    "version should match ECNUVPN_VERSION") && ok;
    }

    // ---- Test 4: service.status delegates to service.helper_status ----
    {
        RpcRequest req;
        req.action = "service.status";
        req.request_id = "5";

        auto resp = dispatcher->dispatch(req);
        ok = expect(resp.success, "service.status should succeed") && ok;

        auto data = json::parse(resp.payload_json);
        ok = expect(data.contains("installed"),
                    "service.status response should contain installed field") && ok;
    }

    // ---- Test 5: drivers.status delegates to service.driver_status ----
    {
        RpcRequest req;
        req.action = "drivers.status";
        req.request_id = "6";

        auto resp = dispatcher->dispatch(req);
        ok = expect(resp.success, "drivers.status should succeed") && ok;

        auto data = json::parse(resp.payload_json);
        ok = expect(data.contains("installed"),
                    "drivers.status response should contain installed field") && ok;
    }

    // ---- Test 6: status callback produces valid JSON ----
    {
        bool callback_called = false;
        json event_json;

        controller->set_status_callback([&](const TunnelStatusSnapshot& snap) {
            callback_called = true;
            event_json = json{
                {"event", "status"},
                {"data",  snapshot_to_json(snap)}
            };
        });

        // Trigger a status change by toggling auto_reconnect
        controller->set_auto_reconnect(false);

        ok = expect(callback_called, "status callback should be called") && ok;
        ok = expect(event_json.contains("event"), "event should contain 'event' key") && ok;
        ok = expect(event_json["event"] == "status", "event type should be 'status'") && ok;
        ok = expect(event_json.contains("data"), "event should contain 'data' key") && ok;
        ok = expect(event_json["data"].contains("phase"), "event data should contain 'phase'") && ok;
    }

    // ---- Test 7: unknown action returns error ----
    {
        RpcRequest req;
        req.action = "nonexistent.action";
        req.request_id = "7";

        auto resp = dispatcher->dispatch(req);
        ok = expect(!resp.success, "unknown action should fail") && ok;
        ok = expect(resp.error_code == "unknown_action",
                    "error code should be unknown_action") && ok;
    }

    // ---- Test 8: VPN action set_auto_reconnect works ----
    {
        controller->set_status_callback(nullptr); // clear callback for this test

        RpcRequest req;
        req.action = "vpn.set_auto_reconnect";
        req.payload_json = R"({"enabled":false})";
        req.request_id = "8";

        auto resp = dispatcher->dispatch(req);
        ok = expect(resp.success, "vpn.set_auto_reconnect should succeed") && ok;

        auto data = json::parse(resp.payload_json);
        ok = expect(data.value("auto_reconnect", true) == false,
                    "auto_reconnect should be false after set") && ok;
    }

    // ---- Test 9: vpn.disconnect is safe when idle ----
    {
        RpcRequest req;
        req.action = "vpn.disconnect";
        req.request_id = "9";

        auto resp = dispatcher->dispatch(req);
        // Disconnect when idle is a no-op but should succeed
        ok = expect(resp.success, "vpn.disconnect when idle should succeed") && ok;
    }

    // ---- Test 10: Wire protocol format simulation ----
    {
        // Simulate the wire format: request JSON -> dispatch -> response JSON
        json request = {
            {"id",     42},
            {"action", "status.get"},
            {"payload", json::object()}
        };

        RpcRequest rpc_req;
        rpc_req.action = request["action"].get<std::string>();
        rpc_req.payload_json = request.contains("payload")
            ? request["payload"].dump() : "{}";
        rpc_req.request_id = std::to_string(request["id"].get<int>());

        auto rpc_resp = dispatcher->dispatch(rpc_req);

        json wire_response;
        if (rpc_resp.success) {
            auto data = json::parse(rpc_resp.payload_json);
            wire_response = json{
                {"id",   request["id"]},
                {"ok",   true},
                {"data", data}
            };
        } else {
            wire_response = json{
                {"id",      request["id"]},
                {"ok",      false},
                {"code",    rpc_resp.error_code},
                {"message", rpc_resp.error_message}
            };
        }

        ok = expect(wire_response.value("id", 0) == 42,
                    "wire response id should match request") && ok;
        ok = expect(wire_response.value("ok", false) == true,
                    "wire response ok should be true") && ok;
        ok = expect(wire_response.contains("data"),
                    "wire response should contain data") && ok;
        ok = expect(wire_response["data"].value("phase", "") == "idle",
                    "wire response data phase should be idle") && ok;
    }

    if (ok) {
        std::cout << "core_process_test: all assertions passed\n";
    } else {
        std::cerr << "core_process_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
