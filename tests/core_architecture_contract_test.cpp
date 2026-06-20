#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>

#ifndef EXV_SOURCE_DIR
#define EXV_SOURCE_DIR "."
#endif

namespace {

bool expect(bool condition, const std::string &message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

bool expect_exists(const std::filesystem::path &path) {
  return expect(std::filesystem::exists(path),
                "Expected path to exist: " + path.string());
}

std::string read_file(const std::filesystem::path &path) {
  std::ifstream input(path);
  std::ostringstream output;
  output << input.rdbuf();
  return output.str();
}

std::size_t count_lines(const std::string &text) {
  if (text.empty()) {
    return 0;
  }
  std::size_t lines = 0;
  for (const char ch : text) {
    if (ch == '\n') {
      ++lines;
    }
  }
  return text.back() == '\n' ? lines : lines + 1;
}

std::size_t count_occurrences(const std::string &text,
                              const std::string &needle) {
  std::size_t count = 0;
  std::size_t pos = 0;
  while ((pos = text.find(needle, pos)) != std::string::npos) {
    ++count;
    pos += needle.size();
  }
  return count;
}

std::string cmake_section(const std::string &cmake,
                          const std::string &section_name) {
  const std::string marker = "set(" + section_name;
  const auto start = cmake.find(marker);
  if (start == std::string::npos) {
    return {};
  }
  const auto end = cmake.find("\n)", start);
  if (end == std::string::npos) {
    return cmake.substr(start);
  }
  return cmake.substr(start, end - start);
}

bool expect_tree_does_not_contain(const std::filesystem::path &root,
                                  const std::string &needle,
                                  const std::string &message) {
  if (!std::filesystem::exists(root)) {
    return true;
  }

  bool ok = true;
  for (const auto &entry :
       std::filesystem::recursive_directory_iterator(root)) {
    if (!entry.is_regular_file()) {
      continue;
    }
    const auto ext = entry.path().extension().string();
    if (ext != ".cpp" && ext != ".hpp" && ext != ".h") {
      continue;
    }
    const std::string content = read_file(entry.path());
    if (content.find(needle) != std::string::npos) {
      ok &= expect(false, message + ": " + entry.path().string());
    }
  }
  return ok;
}

} // namespace

int main() {
  const auto root = std::filesystem::path(EXV_SOURCE_DIR);
  const auto app_api_cpp =
      root / "src" / "core" / "app_api" / "app_api.cpp";
  const auto desktop_action_registry_cpp =
      root / "src" / "core" / "app_api" / "desktop_action_registry.cpp";
  const auto cmake_lists = root / "CMakeLists.txt";
  const auto config_original_cpp =
      root / "src" / "core" / "config" / "config_original.cpp";
  const auto vpn_engine_dir = root / "src" / "vpn_engine";
  const auto cli_dir = root / "src" / "cli";
  bool ok = true;

  ok &= expect_exists(app_api_cpp);
  ok &= expect_exists(desktop_action_registry_cpp);
  ok &= expect_exists(cmake_lists);
  ok &= expect_exists(root / "src" / "core" / "rpc" /
                      "app_rpc_dispatcher.cpp");
  ok &= expect_exists(root / "src" / "core" / "config" /
                      "config_manager.cpp");
  ok &= expect_exists(root / "src" / "vpn_engine" / "native_engine.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_json.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_json.cpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_runtime_context.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_runtime_context.cpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_tunnel_host.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_tunnel_host.cpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_status_presenter.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_status_presenter.cpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_log_actions.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_log_actions.cpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_system_actions.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_system_actions.cpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_route_actions.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_route_actions.cpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_config_actions.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_config_actions.cpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_vpn_actions.hpp");
  ok &= expect_exists(root / "src" / "core" / "app_api" /
                      "desktop_vpn_actions.cpp");

  if (std::filesystem::exists(app_api_cpp)) {
    const std::string app_api = read_file(app_api_cpp);
    ok &= expect(count_lines(app_api) <= 150,
                 "src/core/app_api/app_api.cpp must stay under 150 lines");
    ok &= expect(count_occurrences(app_api, "#include") <= 8,
                 "src/core/app_api/app_api.cpp must have at most 8 includes");
    ok &= expect(app_api.find("register_legacy_handler(") ==
                     std::string::npos,
                 "src/core/app_api/app_api.cpp must not register desktop "
                 "legacy handlers directly");
  }

  if (std::filesystem::exists(desktop_action_registry_cpp)) {
    const std::string desktop_action_registry =
        read_file(desktop_action_registry_cpp);
    ok &= expect(count_lines(desktop_action_registry) <= 80,
                 "src/core/app_api/desktop_action_registry.cpp must stay "
                 "under 80 lines");
    ok &= expect(desktop_action_registry.find("register_legacy_handler(") ==
                     std::string::npos,
                 "desktop action registry must delegate registration to "
                 "action group files");
    ok &= expect(desktop_action_registry.find("preflight_connect(") ==
                     std::string::npos,
                 "desktop action registry must not own VPN behavior");
  }

  const auto core_rpc_dir = root / "src" / "core" / "rpc";
  ok &= expect_tree_does_not_contain(core_rpc_dir, "Stub",
                                    "core/rpc must not expose stub actions");
  ok &= expect_tree_does_not_contain(
      core_rpc_dir, "not_implemented",
      "core/rpc must return explicit real or unsupported behavior, not "
      "not_implemented placeholders");
  ok &= expect_tree_does_not_contain(
      core_rpc_dir, "not yet implemented",
      "core/rpc must not expose not-yet-implemented placeholders");
  ok &= expect_tree_does_not_contain(
      core_rpc_dir, "user_routes_",
      "core/rpc route actions must use persisted config routes");

  ok &= expect(!std::filesystem::exists(config_original_cpp),
               "src/core/config/config_original.cpp must be removed");
  if (std::filesystem::exists(cmake_lists)) {
    const std::string cmake = read_file(cmake_lists);
    ok &= expect(cmake.find("src/core/config/config_original.cpp") ==
                     std::string::npos,
                 "CMakeLists.txt must not compile config_original.cpp");
    const auto cli_sources = cmake_section(cmake, "EXV_CLI_FRONTEND_SOURCES");
    ok &= expect(!cli_sources.empty(),
                 "CMakeLists.txt must declare EXV_CLI_FRONTEND_SOURCES");
    ok &= expect(cmake.find("target_link_libraries(exv-cli PRIVATE exv-core)") ==
                     std::string::npos,
                 "exv-cli must not link the full exv-core implementation target");
    ok &= expect(cli_sources.find("src/core/app_api/") == std::string::npos,
                 "exv-cli sources must not include desktop App API implementations");
    ok &= expect(cli_sources.find("src/app/ui_shell/") == std::string::npos,
                 "exv-cli sources must not include UI shell implementations");
    ok &= expect(cli_sources.find("src/vpn_engine/") == std::string::npos,
                 "exv-cli sources must not include native engine implementations");
    ok &= expect(cli_sources.find("src/core/rpc/") == std::string::npos,
                 "exv-cli sources must not link core RPC handler implementations");
  }

  ok &= expect_tree_does_not_contain(
      vpn_engine_dir, "#include \"core/",
      "vpn_engine must not include higher-level core headers");
  ok &= expect_tree_does_not_contain(
      vpn_engine_dir, "#include <core/",
      "vpn_engine must not include higher-level core headers");
  ok &= expect_tree_does_not_contain(
      vpn_engine_dir, "exv::Config",
      "vpn_engine must not depend on the app Config model");

  ok &= expect_tree_does_not_contain(
      cli_dir, "core/config/config_api",
      "src/cli must not include direct config API headers");
  ok &= expect_tree_does_not_contain(
      cli_dir, "core/config/config_manager",
      "src/cli must not include direct config manager headers");
  ok &= expect_tree_does_not_contain(
      cli_dir, "core/vpn/vpn.hpp",
      "src/cli must not include direct VPN headers");
  ok &= expect_tree_does_not_contain(
      cli_dir, "core/network/virtual_network_status",
      "src/cli must not include direct network status headers");
  ok &= expect_tree_does_not_contain(
      cli_dir, "helper/helper.hpp",
      "src/cli must not include direct helper headers");
  ok &= expect_tree_does_not_contain(
      cli_dir, "app_api::handle_action",
      "src/cli must not call app_api::handle_action directly");

  if (!ok) {
    std::cerr << "core_architecture_contract_test: FAILED\n";
    return 1;
  }

  std::cout << "core_architecture_contract_test: all assertions passed\n";
  return 0;
}
