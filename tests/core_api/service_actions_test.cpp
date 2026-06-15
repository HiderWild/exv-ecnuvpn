// Tests for ServiceActions: helper_status, install, uninstall, driver_status.

#include "core/rpc/service_actions.hpp"
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

struct ServiceActionsFixture {
    exv::core_api::AppRpcDispatcher dispatcher;

    ServiceActionsFixture() {
        exv::core_api::ServiceActions service;
        service.register_handlers(dispatcher);
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

    // --- helper_status returns status ---
    {
        ServiceActionsFixture fix;
        auto resp = fix.dispatch("service.helper_status");
        ok = expect(resp.success, "helper_status should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("installed"), "response missing 'installed'") && ok;
        ok = expect(payload.contains("status"), "response missing 'status'") && ok;
        ok = expect(payload.contains("version"), "response missing 'version'") && ok;

        // Stub returns: installed=false, status="unknown", version=""
        ok = expect(payload["installed"] == false,
                    "stub installed should be false") && ok;
        ok = expect(payload["status"] == "unknown",
                    "stub status should be 'unknown'") && ok;
    }

    // --- install returns not_implemented ---
    {
        ServiceActionsFixture fix;
        auto resp = fix.dispatch("service.install");
        ok = expect(!resp.success, "install should fail (not implemented)") && ok;
        ok = expect(resp.error_code == "not_implemented",
                    "error code should be 'not_implemented'") && ok;
        ok = expect(!resp.error_message.empty(),
                    "error message should not be empty") && ok;
    }

    // --- uninstall returns not_implemented ---
    {
        ServiceActionsFixture fix;
        auto resp = fix.dispatch("service.uninstall");
        ok = expect(!resp.success, "uninstall should fail (not implemented)") && ok;
        ok = expect(resp.error_code == "not_implemented",
                    "error code should be 'not_implemented'") && ok;
        ok = expect(!resp.error_message.empty(),
                    "error message should not be empty") && ok;
    }

    // --- driver_status returns status ---
    {
        ServiceActionsFixture fix;
        auto resp = fix.dispatch("service.driver_status");
        ok = expect(resp.success, "driver_status should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("installed"), "response missing 'installed'") && ok;
        ok = expect(payload.contains("status"), "response missing 'status'") && ok;

        // Stub returns: installed=false, status="unknown"
        ok = expect(payload["installed"] == false,
                    "stub installed should be false") && ok;
        ok = expect(payload["status"] == "unknown",
                    "stub status should be 'unknown'") && ok;
    }

    // --- request_id is propagated ---
    {
        ServiceActionsFixture fix;
        exv::core_api::RpcRequest req;
        req.action = "service.helper_status";
        req.payload_json = "{}";
        req.request_id = "trace-svc-77";
        auto resp = fix.dispatcher.dispatch(req);
        ok = expect(resp.request_id == "trace-svc-77",
                    "request_id should be propagated") && ok;
    }

    if (ok) {
        std::cout << "service_actions_test: all assertions passed\n";
    } else {
        std::cerr << "service_actions_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
