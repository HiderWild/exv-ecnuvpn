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

bool is_regular_file(const std::filesystem::directory_entry &entry) {
  std::error_code ec;
  const bool regular = entry.is_regular_file(ec);
  return !ec && regular;
}

bool is_transient_tree_entry(const std::filesystem::directory_entry &entry) {
  std::error_code ec;
  if (!entry.is_directory(ec) || ec) {
    return false;
  }

  const auto name = entry.path().filename().string();
  return name == ".git" || name == ".reasonix" || name == ".worktrees" ||
         name == "build" || name == "build-windows" || name == "node_modules";
}

template <typename Visitor>
bool scan_tree(const std::filesystem::path &root, Visitor visitor) {
  std::error_code ec;
  if (!std::filesystem::exists(root, ec) || ec ||
      !std::filesystem::is_directory(root, ec) || ec) {
    return false;
  }

  std::filesystem::recursive_directory_iterator it(
      root, std::filesystem::directory_options::skip_permission_denied, ec);
  const std::filesystem::recursive_directory_iterator end;
  while (it != end) {
    if (ec) {
      ec.clear();
      it.increment(ec);
      continue;
    }

    const auto &entry = *it;
    if (is_transient_tree_entry(entry)) {
      it.disable_recursion_pending();
    } else if (visitor(entry)) {
      return true;
    }

    it.increment(ec);
  }
  return false;
}

bool tree_contains_any(const std::filesystem::path &root,
                       const std::vector<std::string> &needles,
                       const char *message) {
  return scan_tree(root, [&](const std::filesystem::directory_entry &entry) {
    if (!is_regular_file(entry) || !is_source_file(entry.path())) {
      return false;
    }
    if (entry.path().filename().string() == "contract_manifest_test.cpp") {
      return false;
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
    return false;
  });
}

bool tree_contains_matching_filename(const std::filesystem::path &root,
                                     std::string_view prefix,
                                     std::string_view suffix) {
  return scan_tree(root, [&](const std::filesystem::directory_entry &entry) {
    if (!is_regular_file(entry)) {
      return false;
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
    return false;
  });
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

std::string relative_slash(const std::filesystem::path &path,
                           const std::filesystem::path &root) {
  return std::filesystem::relative(path, root).generic_string();
}

std::vector<std::string> json_string_array(const json &values) {
  std::vector<std::string> result;
  if (!values.is_array()) {
    return result;
  }
  for (const auto &value : values) {
    if (value.is_string()) {
      result.push_back(value.get<std::string>());
    }
  }
  return result;
}

bool source_root_layout_is_canonical(const std::filesystem::path &source_dir,
                                     const json &src_organization) {
  const auto src_root = source_dir / "src";
  const auto allowed =
      json_string_array(src_organization.at("allowed_top_level_dirs"));
  bool ok = true;
  for (const auto &entry : std::filesystem::directory_iterator(src_root)) {
    const auto name = entry.path().filename().string();
    if (entry.is_regular_file()) {
      std::cerr << "Forbidden root-level src file: "
                << relative_slash(entry.path(), source_dir) << '\n';
      ok = false;
      continue;
    }
    if (!entry.is_directory()) {
      std::cerr << "Unexpected root-level src entry: "
                << relative_slash(entry.path(), source_dir) << '\n';
      ok = false;
      continue;
    }
    if (std::find(allowed.begin(), allowed.end(), name) == allowed.end()) {
      std::cerr << "Unexpected root-level src directory: "
                << relative_slash(entry.path(), source_dir) << '\n';
      ok = false;
    }
  }
  return ok;
}

bool tree_contains_include_unit_file(const std::filesystem::path &root,
                                     const std::filesystem::path &source_dir) {
  return scan_tree(root, [&](const std::filesystem::directory_entry &entry) {
    if (!is_regular_file(entry)) {
      return false;
    }
    if (entry.path().filename().extension() != ".cpp") {
      return false;
    }
    const auto filename = entry.path().filename().string();
    if (filename.size() >= 8 &&
        filename.compare(filename.size() - 8, 8, ".inc.cpp") == 0) {
      std::cerr << "Forbidden source include-unit file: "
                << relative_slash(entry.path(), source_dir) << '\n';
      return true;
    }
    return false;
  });
}

bool tree_contains_include_unit_include(const std::filesystem::path &root,
                                        const std::filesystem::path &source_dir) {
  return scan_tree(root, [&](const std::filesystem::directory_entry &entry) {
    if (!is_regular_file(entry) || !is_source_file(entry.path())) {
      return false;
    }
    if (entry.path().filename().string() == "contract_manifest_test.cpp") {
      return false;
    }
    std::istringstream lines(read_text_file(entry.path()));
    std::string line;
    std::size_t line_number = 0;
    while (std::getline(lines, line)) {
      ++line_number;
      const auto first = line.find_first_not_of(" \t");
      if (first != std::string::npos &&
          line.compare(first, 8, "#include") == 0 &&
          line.find(".inc.cpp", first + 8) != std::string::npos) {
        std::cerr << "Forbidden source include-unit include in "
                  << relative_slash(entry.path(), source_dir) << ':'
                  << line_number << '\n';
        return true;
      }
    }
    return false;
  });
}

bool platform_contains_forbidden_boundary_include(
    const std::filesystem::path &platform_root,
    const std::filesystem::path &source_dir) {
  const std::vector<std::string> forbidden_exact = {
      "app_api.hpp", "vpn.hpp", "tunnel.hpp", "logger.hpp",
      "virtual_network.hpp"};
  return scan_tree(platform_root,
                   [&](const std::filesystem::directory_entry &entry) {
    if (!is_regular_file(entry) || !is_source_file(entry.path())) {
      return false;
    }
    // Platform implementations of core lifecycle headers (PIMPL split moved
    // out of src/core/lifecycle/ to keep core platform-clean) legitimately
    // include their own "core/lifecycle/*.hpp" interface headers.
    const auto rel = relative_slash(entry.path(), source_dir);
    if (rel.rfind("src/platform/common/lifecycle/", 0) == 0) {
      return false;
    }
    std::istringstream lines(read_text_file(entry.path()));
    std::string line;
    std::size_t line_number = 0;
    while (std::getline(lines, line)) {
      ++line_number;
      if (line.find("#include") == std::string::npos) {
        continue;
      }
      for (const auto &name : forbidden_exact) {
        if (line.find('"' + name + '"') != std::string::npos ||
            line.find('<' + name + '>') != std::string::npos) {
          std::cerr << "Forbidden platform boundary include in "
                    << relative_slash(entry.path(), source_dir) << ':'
                    << line_number << ": " << line << '\n';
          return true;
        }
      }
      if (line.find("\"core/") != std::string::npos ||
          line.find("<core/") != std::string::npos ||
          line.find("\"helper/runtime/") != std::string::npos ||
          line.find("<helper/runtime/") != std::string::npos ||
          line.find("\"helper/helper_handler") != std::string::npos ||
          line.find("<helper/helper_handler") != std::string::npos) {
        std::cerr << "Forbidden platform dependency direction in "
                  << relative_slash(entry.path(), source_dir) << ':'
                  << line_number << ": " << line << '\n';
        return true;
      }
    }
    return false;
  });
}

bool private_controller_impl_include_leaks(const std::filesystem::path &root) {
  return scan_tree(root, [&](const std::filesystem::directory_entry &entry) {
    if (!is_regular_file(entry) || !is_source_file(entry.path())) {
      return false;
    }
    if (entry.path().filename().string() == "contract_manifest_test.cpp") {
      return false;
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
      return false;
    }

    const auto relative = std::filesystem::relative(entry.path(), root);
    const auto relative_text = relative.generic_string();
    if (relative_text.rfind("src/core/tunnel_controller/", 0) == 0) {
      return false;
    }

    std::cerr << "Private tunnel controller implementation header leaked into "
              << entry.path().string() << '\n';
    return true;
  });
}

bool source_tree_contains_utils_header_include(const std::filesystem::path &root) {
  return scan_tree(root, [&](const std::filesystem::directory_entry &entry) {
    if (!is_regular_file(entry) || !is_source_file(entry.path())) {
      return false;
    }
    if (entry.path().filename().string() == "contract_manifest_test.cpp") {
      return false;
    }
    const auto text = read_text_file(entry.path());
    if (text.find("#include \"utils.hpp\"") != std::string::npos ||
        text.find("#include <utils.hpp>") != std::string::npos) {
      std::cerr << "Forbidden utils.hpp include in " << entry.path().string()
                << '\n';
      return true;
    }
    return false;
  });
}

bool line_contains_forbidden_utils_scope(const std::string &line) {
  const std::string old_project_namespace = std::string("ecnu") + "vpn";
  if (line.find("namespace utils") != std::string::npos ||
      line.find("namespace " + old_project_namespace + "::utils") !=
          std::string::npos ||
      line.find(old_project_namespace + "::utils::") != std::string::npos) {
    return true;
  }

  std::size_t pos = line.find("utils::");
  while (pos != std::string::npos) {
    const bool qualified_by_exv =
        pos >= 5 && line.compare(pos - 5, 5, "exv::") == 0;
    if (!qualified_by_exv) {
      return true;
    }
    pos = line.find("utils::", pos + 7);
  }
  return false;
}

bool tree_contains_forbidden_utils_scope(const std::filesystem::path &root) {
  return scan_tree(root, [&](const std::filesystem::directory_entry &entry) {
    if (!is_regular_file(entry) || !is_source_file(entry.path())) {
      return false;
    }
    if (entry.path().filename().string() == "contract_manifest_test.cpp") {
      return false;
    }

    std::istringstream lines(read_text_file(entry.path()));
    std::string line;
    std::size_t line_number = 0;
    while (std::getline(lines, line)) {
      ++line_number;
      if (!line_contains_forbidden_utils_scope(line)) {
        continue;
      }
      std::cerr << "Forbidden legacy utils namespace use in "
                << entry.path().string() << ':' << line_number << '\n';
      return true;
    }
    return false;
  });
}

} // namespace

int main() {
  bool ok = true;

  const auto source_dir = std::filesystem::path(EXV_SOURCE_DIR);
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
  const auto legacy_utils_header_path = source_dir / "src" / "utils.hpp";
  const auto tunnel_public_module_path =
      source_dir / "src" / "core" / "tunnel_controller" / "modules" /
      "tunnel.cppm";
  const auto tunnel_state_machine_test =
      read_text_file(source_dir / "tests" /
                     "tunnel_controller_state_machine_test.cpp");
  const auto app_api_cpp =
      source_dir / "src" / "core" / "app_api" / "app_api.cpp";
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
                  app_api_cpp,
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
  ok = expect(contains(exv::contracts::generated::DESKTOP_RPC_EVENT_TYPES,
                       "quick-start-request"),
              "desktop RPC event types must include quick-start-request") &&
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
  ok = expect(
           exv::contracts::generated::is_desktop_rpc_action(
               "vpn.authInteraction.get"),
           "desktop action vpn.authInteraction.get must be generated") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_desktop_rpc_action(
               "vpn.authInteraction.respond"),
           "desktop action vpn.authInteraction.respond must be generated") &&
       ok;
  ok = expect(exv::contracts::generated::is_core_rpc_action("core.hello"),
              "core.hello must be generated as a core RPC action") &&
       ok;
  ok = expect(exv::contracts::generated::is_core_rpc_action("config.export"),
              "config.export must be generated as a core RPC action") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_core_rpc_action("config.getSettings"),
           "config.getSettings must be generated as a core RPC action") &&
       ok;
  ok = expect(exv::contracts::generated::is_core_rpc_action("logs.clear"),
              "logs.clear must be generated as a core RPC action") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_core_rpc_action(
               "maintenance.killStaleCore"),
           "maintenance.killStaleCore must be generated as a core RPC action") &&
       ok;
  ok = expect(exv::contracts::generated::is_core_rpc_action("service.repair"),
              "service.repair must be generated as a core RPC action") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_desktop_rpc_action("service.repair"),
           "service.repair must be generated as a desktop RPC action") &&
       ok;
  ok = expect(exv::contracts::generated::is_core_owned_action("service.repair"),
              "service.repair must have core_rpc ownership") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_destructive_core_rpc_action(
               "config.reset"),
           "config.reset must require explicit confirmation") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_destructive_core_rpc_action(
               "key.reset"),
           "key.reset must require explicit confirmation") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_destructive_core_rpc_action(
               "maintenance.killStaleCore"),
           "maintenance.killStaleCore must require explicit confirmation") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_standard_error_code(
               "core_comm_broken"),
           "core_comm_broken must be a generated standard error") &&
       ok;
  ok = expect(exv::contracts::generated::IPC_PROTOCOL_MAJOR == 1,
              "IPC protocol major must be generated") &&
       ok;
  ok = expect(exv::contracts::generated::is_config_action("config.getSettings"),
              "config.getSettings must be generated as a config action") &&
       ok;
  ok = expect(!exv::contracts::generated::is_config_action("config.getKey"),
              "config.getKey must not be a second config action owner") &&
       ok;
  ok = expect(exv::contracts::generated::is_config_alias("config.get"),
              "legacy config.get must be declared as an alias") &&
       ok;
  ok = expect(exv::contracts::generated::is_core_owned_action("key.status"),
              "key.status must be core owned") &&
       ok;
  ok = expect(exv::contracts::generated::is_compat_alias("config.getKey"),
              "config.getKey must be an alias, not a second owner") &&
       ok;
  ok = expect(exv::contracts::generated::canonical_action_for("config.getKey") ==
                  std::string_view("key.status"),
              "config.getKey must route to key.status") &&
       ok;
  ok = expect(exv::contracts::generated::is_core_owned_action("logs.clear"),
              "logs.clear must have a canonical core owner") &&
       ok;
  for (std::size_t i = 0;
       i < exv::contracts::generated::ACTION_OWNERS.size(); ++i) {
    const auto &left = exv::contracts::generated::ACTION_OWNERS[i];
    for (std::size_t j = i + 1;
         j < exv::contracts::generated::ACTION_OWNERS.size(); ++j) {
      const auto &right = exv::contracts::generated::ACTION_OWNERS[j];
      ok = expect(left.name != right.name,
                  "generated action ownership must contain each action once") &&
           ok;
    }
    if (left.owner == std::string_view("compat_alias")) {
      ok = expect(!left.canonical.empty(),
                  "compat aliases must declare a canonical target") &&
           ok;
      ok = expect(!exv::contracts::generated::is_compat_alias(left.canonical),
                  "compat aliases must not point to another compat alias") &&
           ok;
    }
  }

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
  ok = expect(!std::filesystem::exists(source_dir / "src" /
                                       "utils_terminal.inc.cpp"),
              "terminal output must live under src/cli, not utils include "
              "units") &&
       ok;
  ok = expect(!std::filesystem::exists(source_dir / "src" / "utils.hpp"),
              "monolithic utils.hpp must be removed") &&
       ok;
  ok = expect(!std::filesystem::exists(source_dir / "src" / "utils.cpp"),
              "monolithic utils.cpp must be removed") &&
       ok;
  ok = expect(!tree_contains_matching_filename(source_dir / "src", "utils_",
                                               ".inc.cpp"),
              "utils include-unit sources must be removed") &&
       ok;
  ok = expect(!source_tree_contains_utils_header_include(source_dir / "src"),
              "production code must include explicit utils, cli, or platform "
              "headers instead of utils.hpp") &&
       ok;
  ok = expect(!source_tree_contains_utils_header_include(source_dir / "tests"),
              "tests must include explicit utils, cli, or platform headers "
              "instead of utils.hpp") &&
       ok;
  ok = expect(!tree_contains_forbidden_utils_scope(source_dir / "src"),
              "production code must not use legacy utils namespace or "
              "unqualified utils:: APIs") &&
       ok;
  ok = expect(!tree_contains_forbidden_utils_scope(source_dir / "tests"),
              "tests must not use legacy utils namespace or unqualified "
              "utils:: APIs") &&
       ok;
  if (std::filesystem::exists(legacy_utils_header_path)) {
    ok = expect(!file_contains_any(
                    legacy_utils_header_path,
                    {"print_success", "print_error", "print_info",
                     "print_warning", "print_header", "enable_windows_ansi",
                     "RESET =", "RED =", "GREEN =", "YELLOW =", "BLUE =",
                     "MAGENTA =", "CYAN =", "BOLD =", "DIM =",
                     "UNDERLINE =", "REVERSE ="}),
                "legacy utils.hpp must not expose terminal output APIs or "
                "ANSI constants") &&
         ok;
  }

  ok = expect(manifest.at("modules").contains("src_organization"),
              "manifest must declare modules.src_organization") &&
       ok;
  if (manifest.at("modules").contains("src_organization")) {
    const auto &src_organization = manifest.at("modules").at("src_organization");
    ok = expect(json_array_contains(src_organization.at("allowed_top_level_dirs"),
                                    "platform"),
                "src organization contract must allow platform top-level dir") &&
         ok;
    ok = expect(json_array_contains(src_organization.at("forbidden_patterns"),
                                    "*.inc.cpp"),
                "src organization contract must forbid include-unit files") &&
         ok;
    ok = expect(contains(exv::contracts::generated::SRC_ALLOWED_TOP_LEVEL_DIRS,
                         "vpn_engine"),
                "generated contract must expose allowed src top-level dirs") &&
         ok;
    ok = expect(contains(exv::contracts::generated::SRC_FORBIDDEN_PATTERNS,
                         "src/core_api"),
                "generated contract must expose forbidden src patterns") &&
         ok;
    ok = expect(source_root_layout_is_canonical(source_dir, src_organization),
                "src root must contain only canonical top-level directories") &&
         ok;
  }

  ok = expect(!std::filesystem::exists(source_dir / "src" / "webui_assets.hpp"),
              "webui assets must be generated outside the committed src root") &&
       ok;
  ok = expect(!std::filesystem::exists(source_dir / "src" / "core_api"),
              "core_api compatibility shim directory must be removed") &&
       ok;

  const std::vector<std::filesystem::path> core_tunnel_shims = {
      source_dir / "src" / "core" / "core_error_mapper.hpp",
      source_dir / "src" / "core" / "core_session_runner.hpp",
      source_dir / "src" / "core" / "engine_event_bridge.hpp",
      source_dir / "src" / "core" / "native_startup_failure.hpp",
      source_dir / "src" / "core" / "reconnect_policy.hpp",
      source_dir / "src" / "core" / "timer_scheduler.hpp",
      source_dir / "src" / "core" / "tunnel_controller.hpp",
      source_dir / "src" / "core" / "tunnel_events.hpp",
      source_dir / "src" / "core" / "tunnel_intent.hpp",
      source_dir / "src" / "core" / "tunnel_state.hpp",
  };
  for (const auto &path : core_tunnel_shims) {
    ok = expect(!std::filesystem::exists(path),
                "core root tunnel-controller compatibility shims must be removed") &&
         ok;
  }

  ok = expect(!tree_contains_include_unit_file(source_dir / "src", source_dir),
              "src must not contain .inc.cpp include-unit files") &&
       ok;
  ok = expect(!tree_contains_include_unit_include(source_dir / "src", source_dir),
              "src must not include .inc.cpp implementation units") &&
       ok;
  ok = expect(!platform_contains_forbidden_boundary_include(source_dir / "src" /
                                                               "platform",
                                                           source_dir),
              "platform must not include core/app/helper-runtime or high-level "
              "facade headers") &&
       ok;
  ok = expect(cmake_lists.find("EXV_COMMON_SOURCES") == std::string::npos,
              "CMake must use domain targets instead of EXV_COMMON_SOURCES") &&
       ok;

  if (ok) {
    std::cout << "contract_manifest_test: all assertions passed\n";
  } else {
    std::cerr << "contract_manifest_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
