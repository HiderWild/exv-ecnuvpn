// Tests for ConfigActions as thin RPC adapters over ConfigUseCases.

#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/config_actions.hpp"
#include "contracts/generated/system_contract.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
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
  exv::core_api::ConfigActions config{config_dir};

  ConfigActionsFixture() { config.register_handlers(dispatcher); }

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
    ok = expect(payload["config"]["server"] ==
                    "https://vpn-ct.ecnu.edu.cn",
                "config.get should return default persisted config") &&
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
    ok = expect(payload["config"]["server"] ==
                    "https://vpn.example.edu",
                "saved server should be persisted") &&
         ok;
    ok = expect(payload["config"]["username"] == "alice",
                "saved username should be persisted") &&
         ok;
    ok = expect(payload["settings"]["mtu"] == 1400,
                "saved mtu should be reflected in settings") &&
         ok;
    ok = expect(payload["settings"]["dtls"] == false,
                "saved dtls should be reflected in settings") &&
         ok;
    ok = expect(payload["settings"]["auto_reconnect"] == false,
                "saved auto_reconnect should be reflected in settings") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.save", "{{invalid");
    ok = expect(!resp.success, "config.save with invalid JSON should fail") &&
         ok;
    ok = expect(resp.error_code == "invalid_payload",
                "error code should be invalid_payload") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp =
        fix.dispatch("config.get_profile", R"({"profile_id":"default"})");
    ok = expect(!resp.success,
                "config.get_profile should report unsupported profiles") &&
         ok;
    ok = expect(resp.error_code == "unsupported_action",
                "profile get should return unsupported_action") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp =
        fix.dispatch("config.get_profile", R"({"wrong_field":"x"})");
    ok = expect(!resp.success,
                "get_profile with missing profile_id should fail") &&
         ok;
    ok = expect(resp.error_code == "invalid_payload",
                "error code should be invalid_payload") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch(
        "config.save_profile",
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
