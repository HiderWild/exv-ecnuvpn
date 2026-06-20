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

std::string removed_legacy_engine_value() {
  return std::string("legacy_") + "openconnect";
}

std::string removed_runtime_key() {
  return std::string("openconnect_") + "runtime";
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

  void write_legacy_config() {
    const auto path = std::filesystem::path(config_dir) / "config.json";
    std::ofstream out(path);
    json legacy_config = {
        {"server", "https://legacy.example.edu"},
        {"username", "legacy-user"},
        {"vpn_engine", removed_legacy_engine_value()},
        {removed_runtime_key(), "bundled"},
        {"mtu", 1400},
    };
    out << legacy_config.dump(2);
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
    fix.write_legacy_config();

    auto resp = fix.dispatch("config.get");
    ok = expect(resp.success,
                "config.get should load older legacy config files") &&
         ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["config"]["vpn_engine"] == "native",
                "legacy vpn_engine should normalize to native") &&
         ok;
    ok = expect(!payload["config"].contains(removed_runtime_key()),
                "serialized config should omit retired runtime key") &&
         ok;
    ok = expect(!payload["settings"].contains(removed_runtime_key()),
                "settings projection should omit retired runtime key") &&
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
    auto seed_resp = fix.dispatch(
        "config.saveAuth",
        R"({"server":"https://test.example","username":"bob"})");
    ok = expect(seed_resp.success,
                "config.saveAuth seed should succeed before invalid save") &&
         ok;

    auto resp = fix.dispatch(
        "config.saveAuth",
        R"({"server":"https://test.example","username":"","remember_password":true,"password":"secret"})");
    ok = expect(!resp.success,
                "config.saveAuth should reject remembered password without username") &&
         ok;
    ok = expect(resp.error_code == "invalid_payload",
                "empty username with remembered password should be invalid_payload") &&
         ok;
    ok = expect(resp.error_message.find("username") != std::string::npos,
                "validation error should mention username") &&
         ok;

    auto get_resp = fix.dispatch("config.getAuth");
    auto payload = json::parse(get_resp.payload_json);
    ok = expect(payload["username"] == "bob",
                "invalid auth save should not partially clear username") &&
         ok;
    ok = expect(payload["password_stored"] == false,
                "invalid auth save should not store the submitted password") &&
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
    ok = expect(payload.contains("include_class_a_private_routes"),
                "config.getSettings should include class A private route toggle") &&
         ok;
    ok = expect(payload.contains("include_class_b_private_routes"),
                "config.getSettings should include class B private route toggle") &&
         ok;
    ok = expect(payload.contains("launch_at_login"),
                "config.getSettings should include launch at login toggle") &&
         ok;
    ok = expect(payload.contains("auto_connect_on_launch"),
                "config.getSettings should include auto connect on launch toggle") &&
         ok;
    ok = expect(payload["include_class_a_private_routes"] == false,
                "class A private route toggle should default to false") &&
         ok;
    ok = expect(payload["include_class_b_private_routes"] == false,
                "class B private route toggle should default to false") &&
         ok;
    ok = expect(payload["launch_at_login"] == false,
                "launch at login should default to false") &&
         ok;
    ok = expect(payload["auto_connect_on_launch"] == false,
                "auto connect on launch should default to false") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.saveSettings",
                             R"({"mtu":1500,"dtls":true,"auto_reconnect":true,"include_class_a_private_routes":true,"include_class_b_private_routes":true})");
    ok = expect(resp.success, "config.saveSettings should succeed") && ok;

    auto get_resp = fix.dispatch("config.getSettings");
    auto payload = json::parse(get_resp.payload_json);
    ok = expect(payload["mtu"] == 1500, "saved mtu should be 1500") && ok;
    ok = expect(payload["dtls"] == true, "saved dtls should be true") && ok;
    ok = expect(payload["include_class_a_private_routes"] == true,
                "saved class A private route toggle should be true") &&
         ok;
    ok = expect(payload["include_class_b_private_routes"] == true,
                "saved class B private route toggle should be true") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp =
        fix.dispatch("config.saveSettings",
                     R"({"auto_connect_on_launch":true,"mtu":1200})");
    ok = expect(!resp.success,
                "config.saveSettings should reject auto connect on launch without remembered password and installed service") &&
         ok;
    ok = expect(resp.error_code == "invalid_payload",
                "invalid auto connect on launch should return invalid_payload") &&
         ok;

    auto get_resp = fix.dispatch("config.getSettings");
    auto payload = json::parse(get_resp.payload_json);
    ok = expect(payload["auto_connect_on_launch"] == false,
                "rejected auto connect on launch should remain false") &&
         ok;
    ok = expect(payload["mtu"] == 1290,
                "rejected auto connect on launch should not partially apply mtu") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch(
        "config.saveSettings",
        R"({"dtls":false,"windows_tunnel_driver":"unsupported-driver"})");
    ok = expect(!resp.success,
                "config.saveSettings should reject invalid settings before applying any field") &&
         ok;
    ok = expect(resp.error_code == "invalid_payload",
                "invalid settings should return invalid_payload") &&
         ok;

    auto get_resp = fix.dispatch("config.getSettings");
    auto payload = json::parse(get_resp.payload_json);
    ok = expect(payload["dtls"] == true,
                "invalid settings save should not partially apply earlier valid fields") &&
         ok;
    ok = expect(payload["windows_tunnel_driver"] == "auto",
                "invalid settings save should preserve previous driver choice") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch(
        "config.saveSettings",
        json{{"vpn_engine", removed_legacy_engine_value()}}.dump());
    ok = expect(!resp.success,
                "config.saveSettings should reject legacy vpn_engine") &&
         ok;
    ok = expect(resp.error_code == "invalid_payload",
                "legacy vpn_engine should be an invalid payload") &&
         ok;

    auto get_resp = fix.dispatch("config.getSettings");
    auto payload = json::parse(get_resp.payload_json);
    ok = expect(payload["vpn_engine"] == "native",
                "vpn_engine should remain native after rejected save") &&
         ok;
    ok = expect(!payload.contains(removed_runtime_key()),
                "settings should not expose retired runtime key") &&
         ok;
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
    auto resp = fix.dispatch("config.export");
    ok = expect(resp.success, "config.export should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["exported"] == true, "export should be confirmed") && ok;
    ok = expect(payload.contains("export_data"),
                "export should include export_data") &&
         ok;
    ok = expect(payload["export_data"].contains("server"),
                "export_data should include server") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    fix.dispatch(
        "config.saveAuth",
        R"({"server":"vpn-cn.ecnu.edu.cn","username":"protected-user"})");
    fix.dispatch("config.saveSettings", R"({"mtu":1360,"dtls":false})");

    auto export_resp = fix.dispatch(
        "config.export",
        R"({"protected":true,"password":"correct-password"})");
    ok = expect(export_resp.success,
                "protected config.export should succeed with a password") &&
         ok;
    auto export_payload = json::parse(export_resp.payload_json);
    ok = expect(export_payload["format"] == "protected",
                "protected export should report protected format") &&
         ok;
    ok = expect(export_payload["data"].get<std::string>().find("protected-user") ==
                    std::string::npos,
                "protected export data should not contain plaintext config fields") &&
         ok;

    fix.dispatch(
        "config.saveAuth",
        R"({"server":"vpn-lt.ecnu.edu.cn","username":"changed-user"})");
    fix.dispatch("config.saveSettings", R"({"mtu":1400,"dtls":true})");

    auto wrong_import = fix.dispatch(
        "config.import",
        json{{"format", "protected"},
             {"data", export_payload["data"]},
             {"password", "wrong-password"}}
            .dump());
    ok = expect(!wrong_import.success,
                "protected config.import should reject a wrong password") &&
         ok;
    ok = expect(wrong_import.error_code ==
                    "config_import_tampered_or_wrong_password",
                "wrong protected import password should use auth/tamper error") &&
         ok;
    ok = expect(wrong_import.error_message.find("口令") != std::string::npos,
                "wrong protected import error should mention the export passphrase") &&
         ok;
    ok = expect(wrong_import.error_message.find("损坏") != std::string::npos,
                "wrong protected import error should mention damaged files as a possible cause") &&
         ok;
    ok = expect(wrong_import.error_message.find("用户名") == std::string::npos &&
                    wrong_import.error_message.find("密码") == std::string::npos,
                "wrong protected import error should not blame VPN credentials") &&
         ok;

    auto after_wrong_resp = fix.dispatch("config.get");
    auto after_wrong = json::parse(after_wrong_resp.payload_json);
    ok = expect(after_wrong["config"]["server"] == "vpn-lt.ecnu.edu.cn",
                "wrong password import should not apply protected config") &&
         ok;
    ok = expect(after_wrong["config"]["username"] == "changed-user",
                "wrong password import should not apply protected username") &&
         ok;

    auto correct_import = fix.dispatch(
        "config.import",
        json{{"format", "protected"},
             {"data", export_payload["data"]},
             {"password", "correct-password"}}
            .dump());
    ok = expect(correct_import.success,
                "protected config.import should accept the correct password") &&
         ok;

    auto after_correct_resp = fix.dispatch("config.get");
    auto after_correct = json::parse(after_correct_resp.payload_json);
    ok = expect(after_correct["config"]["server"] == "vpn-cn.ecnu.edu.cn",
                "correct protected import should restore server") &&
         ok;
    ok = expect(after_correct["config"]["username"] == "protected-user",
                "correct protected import should restore username") &&
         ok;
    ok = expect(after_correct["settings"]["mtu"] == 1360,
                "correct protected import should restore settings") &&
         ok;

    fix.dispatch(
        "config.saveAuth",
        R"({"server":"vpn-lt.ecnu.edu.cn","username":"changed-again"})");
    fix.dispatch("config.saveSettings", R"({"mtu":1410,"dtls":true})");

    auto wrapped_correct_import = fix.dispatch(
        "config.import",
        json{{"format", "protected"},
             {"data",
              json{{"format", "protected"}, {"data", export_payload["data"]}}
                  .dump()},
             {"password", "correct-password"}}
            .dump());
    ok = expect(wrapped_correct_import.success,
                "protected config.import should accept exported RPC wrapper files") &&
         ok;

    auto after_wrapped_resp = fix.dispatch("config.get");
    auto after_wrapped = json::parse(after_wrapped_resp.payload_json);
    ok = expect(after_wrapped["config"]["server"] == "vpn-cn.ecnu.edu.cn",
                "wrapped protected import should restore server") &&
         ok;
    ok = expect(after_wrapped["config"]["username"] == "protected-user",
                "wrapped protected import should restore username") &&
         ok;
  }

  {
    ConfigActionsFixture fix;

    // First, modify some settings
    fix.dispatch("config.saveAuth",
                 R"({"server":"https://import-test.example","username":"importuser"})");
    fix.dispatch("config.saveSettings", R"({"mtu":1350,"dtls":false})");

    // Export the config
    auto export_resp = fix.dispatch("config.export");
    ok = expect(export_resp.success, "config.export should succeed") && ok;
    auto export_payload = json::parse(export_resp.payload_json);
    auto export_data = export_payload["export_data"];

    // Reset to defaults
    fix.dispatch("config.reset");

    // Verify reset worked
    auto get_resp = fix.dispatch("config.get");
    auto get_payload = json::parse(get_resp.payload_json);
    ok = expect(get_payload["config"]["server"] != "https://import-test.example",
                "reset should have changed server") &&
         ok;

    // Import back the saved config
    auto import_resp = fix.dispatch("config.import", export_data.dump());
    ok = expect(import_resp.success, "config.import should succeed") && ok;

    // Verify import restored the values
    auto verify_resp = fix.dispatch("config.get");
    auto verify_payload = json::parse(verify_resp.payload_json);
    ok = expect(verify_payload["config"]["server"] == "https://import-test.example",
                "imported server should be restored") &&
         ok;
    ok = expect(verify_payload["config"]["username"] == "importuser",
                "imported username should be restored") &&
         ok;
    ok = expect(verify_payload["settings"]["mtu"] == 1350,
                "imported mtu should be restored") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto resp = fix.dispatch("config.import", "{{invalid}");
    ok = expect(!resp.success,
                "config.import with invalid JSON should fail") &&
         ok;
    ok = expect(resp.error_code == "invalid_payload",
                "error code should be invalid_payload") &&
         ok;
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
