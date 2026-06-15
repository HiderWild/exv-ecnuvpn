// Tests for VpnActions: connect, disconnect, status, set_auto_reconnect.
//
// Uses FakeHelper + FakePlatformNetworkOps to construct a real TunnelController,
// then exercises VpnActions through the AppRpcDispatcher.

#include "core/rpc/vpn_actions.hpp"
#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"

#include "support/fake_helper.hpp"
#include "support/fake_platform_network_ops.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <memory>
#include <string>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

// Helper: build a VpnActions instance wired to a fake TunnelController.
struct VpnActionsFixture {
    std::shared_ptr<exv::test::FakeHelper> helper =
        std::make_shared<exv::test::FakeHelper>();
    std::shared_ptr<exv::test::FakePlatformNetworkOps> net_ops =
        std::make_shared<exv::test::FakePlatformNetworkOps>();
    std::shared_ptr<exv::core::TunnelController> controller;
    std::unique_ptr<exv::core_api::VpnActions> vpn;
    exv::core_api::AppRpcDispatcher dispatcher;

    VpnActionsFixture() {
        controller = std::make_shared<exv::core::TunnelController>(
            helper, net_ops);
        vpn = std::make_unique<exv::core_api::VpnActions>(controller);
        vpn->register_handlers(dispatcher);
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

    // --- connect with valid UserIntent JSON ---
    {
        VpnActionsFixture fix;
        auto resp = fix.dispatch("vpn.connect", R"({
            "profile_id": "default",
            "auto_reconnect": true
        })");

        ok = expect(resp.success, "vpn.connect should succeed") && ok;
        ok = expect(resp.error_code.empty(), "vpn.connect should have no error_code") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("status"), "connect response should contain status") && ok;
        ok = expect(payload["status"] == "connecting",
                    "connect status should be 'connecting'") && ok;
    }

    // --- connect sets controller to non-Idle phase ---
    {
        VpnActionsFixture fix;
        fix.dispatch("vpn.connect", R"({"profile_id":"test","auto_reconnect":false})");

        // After connect, controller should have transitioned through the
        // connect flow.  status() should reflect non-Idle state.
        auto status_resp = fix.dispatch("vpn.status");
        ok = expect(status_resp.success, "vpn.status after connect should succeed") && ok;

        auto s = json::parse(status_resp.payload_json);
        ok = expect(s.contains("phase"), "status should contain phase") && ok;
        // The phase depends on how far the connect flow got; at minimum
        // it should not be "idle" (the connect flow ran).
        ok = expect(s["phase"] != "idle",
                    "phase after connect should not be idle") && ok;
    }

    // --- disconnect ---
    {
        VpnActionsFixture fix;
        // First connect so there is something to disconnect from.
        fix.dispatch("vpn.connect", R"({"profile_id":"default","auto_reconnect":true})");

        auto resp = fix.dispatch("vpn.disconnect");
        ok = expect(resp.success, "vpn.disconnect should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("status"), "disconnect response should contain status") && ok;
        ok = expect(payload["status"] == "disconnecting",
                    "disconnect status should be 'disconnecting'") && ok;
    }

    // --- status returns all expected fields ---
    {
        VpnActionsFixture fix;
        auto resp = fix.dispatch("vpn.status");
        ok = expect(resp.success, "vpn.status should succeed") && ok;

        auto s = json::parse(resp.payload_json);
        ok = expect(s.contains("phase"), "status missing 'phase'") && ok;
        ok = expect(s.contains("desired_connected"), "status missing 'desired_connected'") && ok;
        ok = expect(s.contains("auto_reconnect"), "status missing 'auto_reconnect'") && ok;
        ok = expect(s.contains("helper_mode"), "status missing 'helper_mode'") && ok;
        ok = expect(s.contains("helper_status"), "status missing 'helper_status'") && ok;
        ok = expect(s.contains("network_ready"), "status missing 'network_ready'") && ok;
        ok = expect(s.contains("server"), "status missing 'server'") && ok;
        ok = expect(s.contains("interface_name"), "status missing 'interface_name'") && ok;

        // Initial state: Idle, not connected
        ok = expect(s["phase"] == "idle", "initial phase should be idle") && ok;
        ok = expect(s["desired_connected"] == false,
                    "initial desired_connected should be false") && ok;
    }

    // --- status after connect includes reconnect info when reconnecting ---
    {
        VpnActionsFixture fix;
        fix.dispatch("vpn.connect", R"({"profile_id":"test","auto_reconnect":true})");

        auto resp = fix.dispatch("vpn.status");
        auto s = json::parse(resp.payload_json);

        // The helper's start_session succeeds (FakeHelper), so the flow
        // proceeds past PreparingHelper.  The exact phase depends on the
        // fake helper behavior, but the status should be well-formed.
        ok = expect(s.contains("phase"), "status should have phase") && ok;
        ok = expect(s["phase"].is_string(), "phase should be a string") && ok;
    }

    // --- set_auto_reconnect ---
    {
        VpnActionsFixture fix;
        auto resp = fix.dispatch("vpn.set_auto_reconnect", R"({"enabled":true})");
        ok = expect(resp.success, "set_auto_reconnect(true) should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload["auto_reconnect"] == true,
                    "response should echo auto_reconnect=true") && ok;

        // Disable
        auto resp2 = fix.dispatch("vpn.set_auto_reconnect", R"({"enabled":false})");
        ok = expect(resp2.success, "set_auto_reconnect(false) should succeed") && ok;

        auto payload2 = json::parse(resp2.payload_json);
        ok = expect(payload2["auto_reconnect"] == false,
                    "response should echo auto_reconnect=false") && ok;
    }

    // --- connect with invalid JSON returns error ---
    {
        VpnActionsFixture fix;
        auto resp = fix.dispatch("vpn.connect", "NOT VALID JSON{{{");
        ok = expect(!resp.success, "connect with invalid JSON should fail") && ok;
        ok = expect(resp.error_code == "invalid_payload",
                    "error code should be invalid_payload") && ok;
        ok = expect(!resp.error_message.empty(),
                    "error message should not be empty") && ok;
    }

    // --- set_auto_reconnect with missing 'enabled' field returns error ---
    {
        VpnActionsFixture fix;
        auto resp = fix.dispatch("vpn.set_auto_reconnect", R"({"wrong_field":true})");
        ok = expect(!resp.success, "set_auto_reconnect with missing field should fail") && ok;
        ok = expect(resp.error_code == "invalid_payload",
                    "error code should be invalid_payload") && ok;
    }

    // --- request_id is propagated ---
    {
        VpnActionsFixture fix;
        exv::core_api::RpcRequest req;
        req.action = "vpn.status";
        req.payload_json = "{}";
        req.request_id = "trace-vpn-42";
        auto resp = fix.dispatcher.dispatch(req);
        ok = expect(resp.request_id == "trace-vpn-42",
                    "request_id should be propagated") && ok;
    }

    if (ok) {
        std::cout << "vpn_actions_test: all assertions passed\n";
    } else {
        std::cerr << "vpn_actions_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
