#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>

#ifndef ECNUVPN_SOURCE_DIR
#define ECNUVPN_SOURCE_DIR "."
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

} // namespace

int main() {
  const auto root = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto app_api_cpp =
      root / "src" / "core" / "app_api" / "app_api.cpp";
  bool ok = true;

  ok &= expect_exists(app_api_cpp);
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

  if (!ok) {
    std::cerr << "core_architecture_contract_test: FAILED\n";
    return 1;
  }

  std::cout << "core_architecture_contract_test: all assertions passed\n";
  return 0;
}
