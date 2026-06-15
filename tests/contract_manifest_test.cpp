// Contract manifest and generated C++ snapshot drift test.

#include "contracts/generated/system_contract.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
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

std::string read_text_file(const std::filesystem::path &path) {
  std::ifstream in(path);
  if (!in) {
    throw std::runtime_error("failed to open " + path.string());
  }
  std::ostringstream out;
  out << in.rdbuf();
  return out.str();
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

bool is_source_file(const std::filesystem::path &path) {
  const auto ext = path.extension().string();
  return ext == ".cpp" || ext == ".hpp" || ext == ".h" || ext == ".cppm";
}

bool tree_contains_any(const std::filesystem::path &root,
                       const std::vector<std::string> &needles,
                       const char *message) {
  if (!std::filesystem::exists(root)) {
    return false;
  }
  for (const auto &entry : std::filesystem::recursive_directory_iterator(root)) {
    if (!entry.is_regular_file() || !is_source_file(entry.path())) {
      continue;
    }
    const auto text = read_text_file(entry.path());
    for (const auto &needle : needles) {
      if (text.find(needle) != std::string::npos) {
        std::cerr << "Forbidden dependency in " << entry.path().string()
                  << ": " << needle << '\n';
        std::cerr << message << '\n';
        return true;
      }
    }
  }
  return false;
}

bool tree_contains_matching_filename(const std::filesystem::path &root,
                                     std::string_view prefix,
                                     std::string_view suffix) {
  if (!std::filesystem::exists(root)) {
    return false;
  }
  for (const auto &entry : std::filesystem::recursive_directory_iterator(root)) {
    if (!entry.is_regular_file()) {
      continue;
    }
    const auto filename = entry.path().filename().string();
    if (filename.rfind(prefix, 0) == 0 &&
        filename.size() >= suffix.size() &&
        filename.compare(filename.size() - suffix.size(), suffix.size(),
                         suffix) == 0) {
      std::cerr << "Forbidden tunnel controller include-unit source: "
                << entry.path().string() << '\n';
      return true;
    }
  }
  return false;
}

bool file_contains_any(const std::filesystem::path &path,
                       const std::vector<std::string> &needles) {
  const auto text = read_text_file(path);
  for (const auto &needle : needles) {
    if (text.find(needle) != std::string::npos) {
      std::cerr << "Forbidden text in " << path.string() << ": " << needle
                << '\n';
      return true;
    }
  }
  return false;
}

bool json_array_contains(const json &values, std::string_view expected) {
  if (!values.is_array()) {
    return false;
  }
  for (const auto &value : values) {
    if (value.is_string() && value.get<std::string>() == expected) {
      return true;
    }
  }
  return false;
}

bool private_controller_impl_include_leaks(const std::filesystem::path &root) {
  for (const auto &entry : std::filesystem::recursive_directory_iterator(root)) {
    if (!entry.is_regular_file() || !is_source_file(entry.path())) {
      continue;
    }
    const auto text = read_text_file(entry.path());
    const auto quoted_include =
        std::string{"#include \"core/tunnel_controller/"} +
        "tunnel_controller_impl.hpp\"";
    const auto angle_include =
        std::string{"#include <core/tunnel_controller/"} +
        "tunnel_controller_impl.hpp>";
    if (text.find(quoted_include) == std::string::npos &&
        text.find(angle_include) == std::string::npos) {
      continue;
    }

    const auto relative = std::filesystem::relative(entry.path(), root);
    const auto relative_text = relative.generic_string();
    if (relative_text.rfind("src/core/tunnel_controller/", 0) == 0) {
      continue;
    }

    std::cerr << "Private tunnel controller implementation header leaked into "
              << entry.path().string() << '\n';
    return true;
  }
  return false;
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
  const auto cmake_lists = read_text_file(source_dir / "CMakeLists.txt");
  const auto tunnel_controller_cpp =
      read_text_file(source_dir / "src" / "core" / "tunnel_controller" /
                     "tunnel_controller.cpp");
  const auto tunnel_controller_header =
      read_text_file(source_dir / "src" / "core" / "tunnel_controller" /
                     "tunnel_controller.hpp");
  const auto tunnel_public_module_path =
      source_dir / "src" / "core" / "tunnel_controller" / "modules" /
      "tunnel.cppm";
  const auto tunnel_state_machine_test =
      read_text_file(source_dir / "tests" /
                     "tunnel_controller_state_machine_test.cpp");
  const auto app_api_controller_helpers =
      source_dir / "src" / "core" / "app_api" /
      "app_api_controller_helpers.inc.cpp";
  const auto rpc_vpn_actions =
      source_dir / "src" / "core" / "rpc" / "vpn_actions.cpp";

  ok = expect(manifest == snapshot,
              "generated snapshot must match canonical manifest") &&
       ok;

  ok = expect(!std::filesystem::exists(source_dir / "src" / "core" /
                                       "config" / "platform"),
              "config platform files must live under src/platform, not "
              "src/core/config/platform") &&
       ok;
  ok = expect(cmake_lists.find("src/core/config/platform") ==
                  std::string::npos,
              "CMake must not reference src/core/config/platform") &&
       ok;
  ok = expect(!tree_contains_any(source_dir / "src" / "core" /
                                     "tunnel_controller",
                                 {"platform/win32/", "platform/darwin/",
                                  "platform/linux/"},
                                 "tunnel controller must depend on platform "
                                 "common interfaces, not OS-specific paths"),
              "tunnel controller must not include OS-specific platform paths") &&
       ok;
  ok = expect(!tree_contains_any(source_dir / "src" / "platform",
                                 {"core/tunnel_controller/tunnel_controller",
                                  "core/tunnel_controller/core_session_runner",
                                  "core/tunnel_controller/timer_scheduler",
                                  "core/tunnel_controller/timing",
                                  "core/tunnel_controller/engine_event_bridge"},
                                 "platform must not depend on tunnel "
                                 "controller runtime internals"),
              "platform must not include tunnel controller runtime internals") &&
       ok;
  ok = expect(!tree_contains_any(source_dir / "src" / "helper",
                                 {"core/tunnel_controller/tunnel_controller",
                                  "core/tunnel_controller/core_session_runner",
                                  "core/tunnel_controller/timer_scheduler",
                                  "core/tunnel_controller/timing",
                                  "core/tunnel_controller/engine_event_bridge"},
                                 "helper must not depend on tunnel controller "
                                 "runtime internals"),
              "helper must not include tunnel controller runtime internals") &&
       ok;
  ok = expect(!tree_contains_matching_filename(
                  source_dir / "src" / "core" / "tunnel_controller",
                  "tunnel_controller_", ".inc.cpp"),
              "tunnel controller runtime must use normal private implementation "
              "units, not .inc.cpp include units") &&
       ok;
  ok = expect(tunnel_controller_cpp.find(".inc.cpp") == std::string::npos,
              "tunnel_controller.cpp must not include implementation .inc.cpp "
              "files") &&
       ok;
  ok = expect(tunnel_state_machine_test.find("TestTunnelController") ==
                  std::string::npos,
              "tunnel_controller_state_machine_test must exercise the real "
              "TunnelController, not a mirror state machine") &&
       ok;
  ok = expect(tunnel_controller_header.find("NativeDependenciesFactory") ==
                  std::string::npos,
              "public TunnelController header must not expose native dependency "
              "test seams") &&
       ok;
  ok = expect(tunnel_controller_header.find("core_session_runner") ==
                  std::string::npos,
              "public TunnelController header must not include native session "
              "runner internals") &&
       ok;
  ok = expect(!private_controller_impl_include_leaks(source_dir),
              "tunnel_controller_impl.hpp must stay private to "
              "src/core/tunnel_controller implementation units") &&
       ok;
  ok = expect(!file_contains_any(
                  app_api_controller_helpers,
                  {"phase_to_string", "return \"idle\"",
                   "return \"preparing_helper\"",
                   "return \"applying_network_config\""}),
              "app_api must use the centralized tunnel phase wire-name helper") &&
       ok;
  ok = expect(!file_contains_any(
                  rpc_vpn_actions,
                  {"phase_to_string", "phase_str = \"idle\"",
                   "phase_str = \"preparing_helper\"",
                   "phase_str = \"applying_network_config\""}),
              "vpn_actions must use the centralized tunnel phase wire-name "
              "helper") &&
       ok;
  ok = expect(std::filesystem::exists(tunnel_public_module_path),
              "public tunnel module exv.core.tunnel must exist") &&
       ok;
  if (std::filesystem::exists(tunnel_public_module_path)) {
    const auto tunnel_public_module = read_text_file(tunnel_public_module_path);
    ok = expect(tunnel_public_module.find("NativeDependenciesFactory") ==
                    std::string::npos,
                "public tunnel module must not export native dependency test "
                "seams") &&
         ok;
    ok = expect(tunnel_public_module.find("native_engine") ==
                    std::string::npos,
                "public tunnel module must not include native engine details") &&
         ok;
    ok = expect(tunnel_public_module.find("helper/common") ==
                    std::string::npos,
                "public tunnel module must not include helper wire details") &&
         ok;
    ok = expect(tunnel_public_module.find("platform/common") ==
                    std::string::npos,
                "public tunnel module must not include platform implementation "
                "details") &&
         ok;
  }

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

  ok = expect(exv::contracts::generated::is_helper_op("StartSession"),
              "helper StartSession op must be generated") &&
       ok;
  ok = expect(exv::contracts::generated::is_helper_op("ApplyTunnelConfig"),
              "helper ApplyTunnelConfig op must be generated") &&
       ok;
  ok = expect(exv::contracts::generated::is_helper_op("Shutdown"),
              "helper Shutdown op must be generated") &&
       ok;
  ok = expect(!exv::contracts::generated::is_helper_op("EndSession"),
              "helper EndSession op must not be generated") &&
       ok;
  for (const auto &op : exv::contracts::generated::HELPER_OP_CONTRACTS) {
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
  ok = expect(manifest.at("modules").contains("tunnel_controller"),
              "manifest must declare modules.tunnel_controller") &&
       ok;
  ok = expect(manifest.at("modules").contains("utils"),
              "manifest must declare modules.utils") &&
       ok;
  if (manifest.at("modules").contains("utils")) {
    const auto &utils = manifest.at("modules").at("utils");
    ok = expect(json_array_contains(utils.at("boundary").at("accepts"),
                                    "pure string values"),
                "utils boundary must accept only pure values") &&
         ok;
    ok = expect(json_array_contains(utils.at("boundary").at("rejects"),
                                    "filesystem access"),
                "utils boundary must reject filesystem access") &&
         ok;
  }
  ok = expect(!std::filesystem::exists(source_dir / "src" / "utils_platform"),
              "utils must not grow a platform subdirectory") &&
       ok;

  if (ok) {
    std::cout << "contract_manifest_test: all assertions passed\n";
  } else {
    std::cerr << "contract_manifest_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
