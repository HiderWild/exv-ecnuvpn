#include <filesystem>
#include <iostream>
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

} // namespace

int main() {
  const auto root = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  bool ok = true;

  ok &= expect_exists(root / "src" / "core" / "app_api" / "app_api.cpp");
  ok &= expect_exists(root / "src" / "core" / "rpc" /
                      "app_rpc_dispatcher.cpp");
  ok &= expect_exists(root / "src" / "core" / "config" /
                      "config_manager.cpp");
  ok &= expect_exists(root / "src" / "vpn_engine" / "native_engine.hpp");

  if (!ok) {
    std::cerr << "core_architecture_contract_test: FAILED\n";
    return 1;
  }

  std::cout << "core_architecture_contract_test: all assertions passed\n";
  return 0;
}
