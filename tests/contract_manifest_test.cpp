// Contract manifest and generated C++ snapshot drift test.

#include "contracts/generated/system_contract.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <string_view>
#include <vector>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

json read_json_file(const std::filesystem::path &path) {
  std::ifstream in(path);
  if (!in) {
    throw std::runtime_error("failed to open " + path.string());
  }
  json parsed;
  in >> parsed;
  return parsed;
}

template <typename Range>
bool contains(const Range &values, std::string_view value) {
  return std::find(values.begin(), values.end(), value) != values.end();
}

bool alias_targets_exist(const json &aliases, const json &actions) {
  std::vector<std::string> action_names;
  for (const auto &action : actions) {
    action_names.push_back(action.at("name").get<std::string>());
  }
  for (const auto &alias : aliases) {
    const auto target = alias.at("target").get<std::string>();
    if (std::find(action_names.begin(), action_names.end(), target) ==
        action_names.end()) {
      std::cerr << "Alias target is not a config action: " << target << '\n';
      return false;
    }
  }
  return true;
}

} // namespace

int main() {
  bool ok = true;

  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto manifest =
      read_json_file(source_dir / "contracts" / "system.contract.json");
  const auto snapshot =
      read_json_file(source_dir / "contracts" / "generated" /
                     "system_contract_snapshot.json");

  ok = expect(manifest == snapshot,
              "generated snapshot must match canonical manifest") &&
       ok;

  ok = expect(std::string(exv::contracts::generated::CONTRACT_VERSION) ==
                  manifest.at("version").get<std::string>(),
              "generated contract version must match manifest") &&
       ok;

  ok = expect(contains(exv::contracts::generated::DESKTOP_RPC_REQUEST_FIELDS,
                       "id"),
              "desktop RPC request must include id") &&
       ok;
  ok = expect(contains(exv::contracts::generated::DESKTOP_RPC_REQUEST_FIELDS,
                       "action"),
              "desktop RPC request must include action") &&
       ok;
  ok = expect(contains(exv::contracts::generated::DESKTOP_RPC_REQUEST_FIELDS,
                       "payload"),
              "desktop RPC request must include payload") &&
       ok;

  ok = expect(contains(exv::contracts::generated::DESKTOP_RPC_RESPONSE_FIELDS,
                       "ok"),
              "desktop RPC response must include ok") &&
       ok;
  ok = expect(contains(exv::contracts::generated::DESKTOP_RPC_RESPONSE_FIELDS,
                       "event"),
              "desktop RPC response must include event") &&
       ok;

  ok = expect(contains(exv::contracts::generated::CORE_RPC_REQUEST_FIELDS,
                       "payload_json"),
              "core RPC request must include payload_json") &&
       ok;
  ok = expect(contains(exv::contracts::generated::CORE_RPC_RESPONSE_FIELDS,
                       "error_code"),
              "core RPC response must include error_code") &&
       ok;

  ok = expect(exv::contracts::generated::is_desktop_rpc_action("config.getAuth"),
              "desktop action config.getAuth must be generated") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_desktop_rpc_action("config.saveSettings"),
           "desktop action config.saveSettings must be generated") &&
       ok;
  ok = expect(exv::contracts::generated::is_config_action("config.getSettings"),
              "config.getSettings must be generated as a config action") &&
       ok;
  ok = expect(exv::contracts::generated::is_config_alias("config.get"),
              "legacy config.get must be declared as an alias") &&
       ok;

  ok = expect(exv::contracts::generated::is_helper_v2_op("StartSession"),
              "helper StartSession op must be generated") &&
       ok;
  ok = expect(exv::contracts::generated::is_helper_v2_op("ApplyTunnelConfig"),
              "helper ApplyTunnelConfig op must be generated") &&
       ok;
  for (const auto &op : exv::contracts::generated::HELPER_V2_OP_CONTRACTS) {
    if (op.name == "StartSession") {
      ok = expect(op.code == 2, "StartSession op code must be generated") && ok;
      ok = expect(!op.requires_session,
                  "StartSession requires_session must be generated") &&
           ok;
    }
  }
  ok = expect(exv::contracts::generated::is_helper_forbidden_credential_field(
                  "password"),
              "helper password field must be forbidden") &&
       ok;
  ok = expect(exv::contracts::generated::is_helper_forbidden_credential_field(
                  "auth_token"),
              "helper auth_token field must be forbidden") &&
       ok;

  const auto &config = manifest.at("modules").at("config");
  ok = expect(alias_targets_exist(config.at("aliases"), config.at("actions")),
              "every config alias target must be an action") &&
       ok;

  if (ok) {
    std::cout << "contract_manifest_test: all assertions passed\n";
  } else {
    std::cerr << "contract_manifest_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
