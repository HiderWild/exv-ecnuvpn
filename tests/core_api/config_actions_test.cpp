// Tests for ConfigActions: get, save, get_profile, save_profile.

#include "core_api/config_actions.hpp"
#include "core_api/app_rpc_dispatcher.hpp"
#include "contracts/generated/system_contract.hpp"

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

struct ConfigActionsFixture {
    exv::core_api::AppRpcDispatcher dispatcher;

    ConfigActionsFixture() {
        exv::core_api::ConfigActions config;
        config.register_handlers(dispatcher);
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

    // --- config.get returns config ---
    {
        ConfigActionsFixture fix;
        auto resp = fix.dispatch("config.get");
        ok = expect(resp.success, "config.get should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("config"), "response should contain 'config'") && ok;
        ok = expect(payload["config"].is_object(), "config should be an object") && ok;
    }

    // --- config.save acknowledges ---
    {
        ConfigActionsFixture fix;
        auto resp = fix.dispatch("config.save", R"({"settings":{"theme":"dark"}})");
        ok = expect(resp.success, "config.save should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("saved"), "response should contain 'saved'") && ok;
        ok = expect(payload["saved"] == true, "saved should be true") && ok;
    }

    // --- config.save with invalid JSON returns error ---
    {
        ConfigActionsFixture fix;
        auto resp = fix.dispatch("config.save", "{{invalid");
        ok = expect(!resp.success, "config.save with invalid JSON should fail") && ok;
        ok = expect(resp.error_code == "invalid_payload",
                    "error code should be invalid_payload") && ok;
    }

    // --- config.get_profile returns profile ---
    {
        ConfigActionsFixture fix;
        auto resp = fix.dispatch("config.get_profile", R"({"profile_id":"default"})");
        ok = expect(resp.success, "config.get_profile should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("profile_id"), "response should contain 'profile_id'") && ok;
        ok = expect(payload["profile_id"] == "default",
                    "profile_id should match request") && ok;
        ok = expect(payload.contains("data"), "response should contain 'data'") && ok;
    }

    // --- config.get_profile with missing profile_id returns error ---
    {
        ConfigActionsFixture fix;
        auto resp = fix.dispatch("config.get_profile", R"({"wrong_field":"x"})");
        ok = expect(!resp.success, "get_profile with missing profile_id should fail") && ok;
        ok = expect(resp.error_code == "invalid_payload",
                    "error code should be invalid_payload") && ok;
    }

    // --- config.save_profile acknowledges ---
    {
        ConfigActionsFixture fix;
        auto resp = fix.dispatch("config.save_profile",
                                 R"({"profile_id":"work","data":{"server":"vpn.work.com"}})");
        ok = expect(resp.success, "config.save_profile should succeed") && ok;

        auto payload = json::parse(resp.payload_json);
        ok = expect(payload.contains("profile_id"), "response should contain 'profile_id'") && ok;
        ok = expect(payload["profile_id"] == "work",
                    "profile_id should match request") && ok;
        ok = expect(payload.contains("saved"), "response should contain 'saved'") && ok;
        ok = expect(payload["saved"] == true, "saved should be true") && ok;
    }

    // --- config.save_profile with missing profile_id returns error ---
    {
        ConfigActionsFixture fix;
        auto resp = fix.dispatch("config.save_profile", R"({"data":{}})");
        ok = expect(!resp.success, "save_profile with missing profile_id should fail") && ok;
        ok = expect(resp.error_code == "invalid_payload",
                    "error code should be invalid_payload") && ok;
    }

    // --- request_id is propagated ---
    {
        ConfigActionsFixture fix;
        exv::core_api::RpcRequest req;
        req.action = "config.get";
        req.payload_json = "{}";
        req.request_id = "trace-config-99";
        auto resp = fix.dispatcher.dispatch(req);
        ok = expect(resp.request_id == "trace-config-99",
                    "request_id should be propagated") && ok;
    }

    // --- contract manifest declares config actions and aliases ---
    {
        using namespace exv::contracts::generated;
        ok = expect(is_config_action("config.getAuth"),
                    "manifest should declare config.getAuth") && ok;
        ok = expect(is_config_action("config.saveSettings"),
                    "manifest should declare config.saveSettings") && ok;
        ok = expect(is_config_action("config.profile.get"),
                    "manifest should declare config.profile.get") && ok;
        ok = expect(is_config_alias("config.get"),
                    "manifest should declare legacy config.get alias") && ok;
        ok = expect(is_config_alias("config.save_profile"),
                    "manifest should declare legacy config.save_profile alias") && ok;
    }

    if (ok) {
        std::cout << "config_actions_test: all assertions passed\n";
    } else {
        std::cerr << "config_actions_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
