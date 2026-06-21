// Tests for ConfigActions as thin RPC adapters over ConfigUseCases.

#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/config_actions.hpp"
#include "core/crypto/crypto.hpp"
#include "contracts/generated/system_contract.hpp"
#include "platform/common/runtime_paths.hpp"

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
    exv::platform::set_runtime_path_override(config_dir, config_dir);
    config.register_handlers(dispatcher);
  }

  ~ConfigActionsFixture() {
    exv::platform::clear_runtime_path_override();
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

  json read_config_file() const {
    const auto path = std::filesystem::path(config_dir) / "config.json";
    std::ifstream in(path);
    return json::parse(in);
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

    auto tampered_kdf = json::parse(export_payload["data"].get<std::string>());
    tampered_kdf["kdf"] = "unexpected-kdf";
    auto tampered_kdf_import = fix.dispatch(
        "config.import",
        json{{"format", "protected"},
             {"data", tampered_kdf.dump()},
             {"password", "correct-password"}}
            .dump());
    ok = expect(!tampered_kdf_import.success,
                "protected config.import should reject tampered KDF metadata") &&
         ok;
    ok = expect(tampered_kdf_import.error_code ==
                    "config_import_tampered_or_wrong_password",
                "tampered protected metadata should use auth/tamper error") &&
         ok;
    auto after_tampered_kdf_resp = fix.dispatch("config.get");
    auto after_tampered_kdf = json::parse(after_tampered_kdf_resp.payload_json);
    ok = expect(after_tampered_kdf["config"]["server"] == "vpn-lt.ecnu.edu.cn",
                "tampered KDF import should not apply protected config") &&
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
    auto save_resp = fix.dispatch(
        "config.saveAuth",
        R"({"server":"https://secret.example","username":"secret-user","remember_password":true,"password":"vpn-secret"})");
    ok = expect(save_resp.success,
                "config.saveAuth should store a remembered VPN password") &&
         ok;

    auto auth_resp = fix.dispatch("config.getAuth");
    auto auth_payload = json::parse(auth_resp.payload_json);
    ok = expect(auth_payload["password_stored"] == true,
                "saved auth should report a stored VPN password") &&
         ok;

    auto export_resp = fix.dispatch(
        "config.export",
        R"({"protected":true,"password":"export-passphrase"})");
    ok = expect(export_resp.success,
                "protected export should include saved VPN credentials") &&
         ok;
    auto export_payload = json::parse(export_resp.payload_json);
    const std::string downloaded_file =
        export_payload["data"].get<std::string>();
    ok = expect(downloaded_file.find("vpn-secret") == std::string::npos,
                "protected export file should not expose the VPN password") &&
         ok;

    auto clear_resp = fix.dispatch(
        "config.saveAuth",
        R"({"server":"https://changed.example","username":"changed-user","remember_password":false})");
    ok = expect(clear_resp.success,
                "test should clear saved credentials before import") &&
         ok;
    ok = expect(exv::crypto::key_status() == "missing",
                "clearing remembered credentials should remove the local key") &&
         ok;

    auto wrong_import = fix.dispatch(
        "config.import",
        json{{"format", "protected"},
             {"data", downloaded_file},
             {"password", "wrong-export-passphrase"}}
            .dump());
    ok = expect(!wrong_import.success,
                "protected import should reject the wrong export passphrase") &&
         ok;

    auto export_after_wrong = fix.dispatch("config.export");
    ok = expect(export_after_wrong.success,
                "config.export should still respond after a failed protected import") &&
         ok;

    auto correct_import = fix.dispatch(
        "config.import",
        json{{"format", "protected"},
             {"data", downloaded_file},
             {"password", "export-passphrase"}}
            .dump());
    ok = expect(correct_import.success,
                "protected import should accept the correct export passphrase after a failure") &&
         ok;

    auto imported_auth_resp = fix.dispatch("config.getAuth");
    auto imported_auth = json::parse(imported_auth_resp.payload_json);
    ok = expect(imported_auth["username"] == "secret-user",
                "protected import should restore the auth username") &&
         ok;
    ok = expect(imported_auth["password_stored"] == true,
                "protected import should recreate local storage for the VPN password") &&
         ok;

    auto on_disk = fix.read_config_file();
    const std::string stored_ciphertext =
        on_disk.value("password", std::string());
    ok = expect(!stored_ciphertext.empty() &&
                    stored_ciphertext != "vpn-secret",
                "imported VPN password should be stored encrypted") &&
         ok;
    ok = expect(exv::crypto::decrypt(stored_ciphertext,
                                     exv::crypto::load_key()) ==
                    "vpn-secret",
                "imported encrypted VPN password should decrypt correctly") &&
         ok;
  }

  {
    ConfigActionsFixture fix;
    auto save_resp = fix.dispatch(
        "config.saveAuth",
        R"({"server":"https://secret.example","username":"secret-user","remember_password":true,"password":"vpn-secret"})");
    ok = expect(save_resp.success,
                "config.saveAuth should seed a remembered password before unprotected export") &&
         ok;

    auto export_resp = fix.dispatch("config.export", R"({"protected":false})");
    ok = expect(export_resp.success,
                "unprotected config.export should succeed with saved credentials") &&
         ok;
    auto export_payload = json::parse(export_resp.payload_json);
    auto exported_file = json::parse(export_payload["data"].get<std::string>());
    ok = expect(exported_file.value("password", std::string()).empty(),
                "unprotected export should not contain the VPN password") &&
         ok;
    ok = expect(exported_file.value("remember_password", true) == false,
                "unprotected export should not preserve remember_password=true") &&
         ok;
    ok = expect(exported_file.value("password_stored", true) == false,
                "unprotected export should not claim a password is stored") &&
         ok;

    auto replace_resp = fix.dispatch(
        "config.saveAuth",
        R"({"server":"https://changed.example","username":"changed-user","remember_password":true,"password":"local-secret"})");
    ok = expect(replace_resp.success,
                "test should store a local password before unprotected import") &&
         ok;
    ok = expect(exv::crypto::key_status() == "valid",
                "test should have a local key before unprotected import") &&
         ok;

    auto import_resp = fix.dispatch(
        "config.import",
        json{{"format", "unprotected"}, {"data", export_payload["data"]}}
            .dump());
    ok = expect(import_resp.success,
                "unprotected import should accept the redacted export file") &&
         ok;

    auto imported_auth_resp = fix.dispatch("config.getAuth");
    auto imported_auth = json::parse(imported_auth_resp.payload_json);
    ok = expect(imported_auth["remember_password"] == false,
                "unprotected import should clear remember_password") &&
         ok;
    ok = expect(imported_auth["password_stored"] == false,
                "unprotected import should clear existing local password storage") &&
         ok;
    ok = expect(exv::crypto::key_status() == "missing",
                "unprotected import should remove the local encryption key") &&
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
    fix.dispatch("config.saveSettings", R"({"mtu":1350})");
    auto resp = fix.dispatch("config.import", R"({"mtu":"not-a-number"})");
    ok = expect(!resp.success,
                "config.import with an invalid field type should fail") &&
         ok;
    ok = expect(resp.error_code == "invalid_config",
                "invalid imported field type should be invalid_config") &&
         ok;

    auto after_invalid = json::parse(fix.dispatch("config.get").payload_json);
    ok = expect(after_invalid["settings"]["mtu"] == 1350,
                "invalid field type import should leave existing settings unchanged") &&
         ok;

    auto export_after_invalid = fix.dispatch("config.export");
    ok = expect(export_after_invalid.success,
                "config.export should still respond after invalid config import") &&
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
