// Tests for ConfigActions as thin RPC adapters over ConfigUseCases.

#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/config_actions.hpp"
#include "contracts/generated/system_contract.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::string unique_temp_dir(const char *name) {
  const auto now = std::chrono::steady_clock::now().time_since_epoch().count();
  auto dir = std::filesystem::temp_directory_path() /
             (std::string(name) + "-" + std::to_string(now));
  std::filesystem::create_directories(dir);
  return dir.string();
}

struct ConfigActionsFixture {
  std::string config_dir = unique_temp_dir("exv-config-actions-test");
  exv::core_api::AppRpcDispatcher dispatcher;
  exv::core_api::ConfigActions config;

  ConfigActionsFixture()
      : config(config_dir) {
    config.register_handlers(dispatcher);
  }

  ~ConfigActionsFixture() {
    std::error_code ec;
    std::filesystem::remove_all(config_dir, ec);
  }

  exv::core_api::RpcResponse dispatch(const std::string &action,
                                      const std::string &payload = "{}") {
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

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.get");
    ok = expect(resp.success, "config.get should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("config"), "response should contain config") &&
         ok;
    ok = expect(payload.contains("settings"),
                "response should contain settings projection") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch(
        "config.save",
        R"({"server":"https://vpn.example.edu","username":"alice","settings":{"mtu":1400,"dtls":false,"auto_reconnect":false}})");
    ok = expect(resp.success, "config.save should persist supported fields") &&
         ok;

    auto get_resp = fix.dispatch("config.get");
    auto payload = json::parse(get_resp.payload_json);
    ok = expect(payload["config"]["server"] == "https://vpn.example.edu",
                "saved server should be persisted") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.save", "{{invalid}");
    ok = expect(!resp.success, "config.save with invalid JSON should fail") &&
         ok;
    ok = expect(resp.error_code == "invalid_payload",
                "error code should be invalid_payload") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.getAuth");
    ok = expect(resp.success, "config.getAuth should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("server"),
                "config.getAuth should return server") &&
         ok;
    ok = expect(payload.contains("username"),
                "config.getAuth should return username") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.saveAuth",
                             R"({"server":"https://test.example","username":"bob"})");
    ok = expect(resp.success, "config.saveAuth should succeed") && ok;

    auto get_resp = fix.dispatch("config.getAuth");
    auto payload = json::parse(get_resp.payload_json);
    ok = expect(payload["server"] == "https://test.example",
                "saved auth server should persist") &&
         ok;
    ok = expect(payload["username"] == "bob",
                "saved auth username should persist") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.getSettings");
    ok = expect(resp.success, "config.getSettings should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("mtu"),
                "config.getSettings should include mtu") &&
         ok;
    ok = expect(payload.contains("dtls"),
                "config.getSettings should include dtls") &&
         ok;
    ok = expect(payload.contains("auto_reconnect"),
                "config.getSettings should include auto_reconnect") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.saveSettings",
                             R"({"mtu":1500,"dtls":true,"auto_reconnect":true})");
    ok = expect(resp.success, "config.saveSettings should succeed") && ok;

    auto get_resp = fix.dispatch("config.getSettings");
    auto payload = json::parse(get_resp.payload_json);
    ok = expect(payload["mtu"] == 1500, "saved mtu should be 1500") && ok;
    ok = expect(payload["dtls"] == true, "saved dtls should be true") && ok;
  }

  {
    ConfigActionsFixture fix;
    // Make a change first
    fix.dispatch("config.saveSettings", R"({"mtu":1400})");

    // Reset config
    auto resp = fix.dispatch("config.reset");
    ok = expect(resp.success, "config.reset should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["reset"] == true, "reset should be confirmed") && ok;
    ok = expect(payload["settings"]["mtu"] == 1290,
                "reset should restore default mtu") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("key.status");
    ok = expect(resp.success, "key.status should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("present"), "should indicate key presence") &&
         ok;
    ok = expect(payload.contains("status"), "should include key status") && ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("key.reset");
    ok = expect(resp.success, "key.reset should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["reset"] == true, "key reset should be confirmed") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("routes.list");
    ok = expect(resp.success, "routes.list should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("routes"),
                "routes.list should return routes array") &&
         ok;
    ok = expect(payload["routes"].is_array(), "routes should be array") && ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("routes.add", R"({"cidr":"10.0.0.0/24"})");
    ok = expect(resp.success, "routes.add should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["added"] == true, "add should be confirmed") && ok;

    // Verify it's in the list
    auto list_resp = fix.dispatch("routes.list");
    auto list_payload = json::parse(list_resp.payload_json);
    bool found = false;
    for (const auto& r : list_payload["routes"]) {
      if (r["cidr"] == "10.0.0.0/24") {
        found = true;
        break;
      }
    }
    ok = expect(found, "added route should be in list") && ok;
  }

  {
    ConfigActionsFixture fix;
    // Add a route first
    fix.dispatch("routes.add", R"({"cidr":"10.0.0.0/24"})");

    auto resp = fix.dispatch("routes.remove", R"({"cidr":"10.0.0.0/24"})");
    ok = expect(resp.success, "routes.remove should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["removed"] == true, "remove should be confirmed") && ok;

    // Verify it's gone
    auto list_resp = fix.dispatch("routes.list");
    auto list_payload = json::parse(list_resp.payload_json);
    bool found = false;
    for (const auto& r : list_payload["routes"]) {
      if (r["cidr"] == "10.0.0.0/24") {
        found = true;
        break;
      }
    }
    ok = expect(!found, "removed route should not be in list") && ok;
  }

  {
    ConfigActionsFixture fix;
    // Add a custom route first
    fix.dispatch("routes.add", R"({"cidr":"10.0.0.0/24"})");

    auto resp = fix.dispatch("routes.reset");
    ok = expect(resp.success, "routes.reset should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["reset"] == true, "reset should be confirmed") && ok;

    // Verify custom route is gone
    auto list_resp = fix.dispatch("routes.list");
    auto list_payload = json::parse(list_resp.payload_json);
    bool found = false;
    for (const auto& r : list_payload["routes"]) {
      if (r["cidr"] == "10.0.0.0/24") {
        found = true;
        break;
      }
    }
    ok = expect(!found, "custom route should be gone after reset") && ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.get_profile", R"({"profile_id":"default"})");
    ok = expect(!resp.success,
                "config.get_profile should report unsupported profiles") &&
         ok;
    ok = expect(resp.error_code == "unsupported_action",
                "profile get should return unsupported_action") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.save_profile",
                             R"({"profile_id":"work","data":{"server":"vpn.work.com"}})");
    ok = expect(!resp.success,
                "config.save_profile should report unsupported profiles") &&
         ok;
    ok = expect(resp.error_code == "unsupported_action",
                "profile save should return unsupported_action") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    exv::core_api::RpcRequest req;
    req.action = "config.get";
    req.payload_json = "{}";
    req.request_id = "trace-config-99";
    auto resp = fix.dispatcher.dispatch(req);
    ok = expect(resp.request_id == "trace-config-99",
                "request_id should be propagated") &&
         ok;
  }

  {
    using namespace exv::contracts::generated;
    ok = expect(is_config_action("config.getAuth"),
                "manifest should declare config.getAuth") &&
         ok;
    ok = expect(is_config_action("config.saveSettings"),
                "manifest should declare config.saveSettings") &&
         ok;
    ok = expect(is_config_action("config.profile.get"),
                "manifest should declare config.profile.get") &&
         ok;
    ok = expect(is_config_alias("config.get"),
                "manifest should declare legacy config.get alias") &&
         ok;
    ok = expect(is_config_alias("config.save_profile"),
                "manifest should declare legacy config.save_profile alias") &&
         ok;
  }

  if (ok) {
    std::cout << "config_actions_test: all assertions passed\n";
  } else {
    std::cerr << "config_actions_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
