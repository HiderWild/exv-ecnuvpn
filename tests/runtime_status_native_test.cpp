#include "core/config/config.hpp"
#include "core/config/config_platform_view.hpp"
#include "platform/common/runtime_status.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

} // namespace

namespace ecnuvpn {
namespace platform {

std::string run_command_output(const std::string &) { return ""; }
std::string shell_quote(const std::string &value) { return value; }
std::string get_bundled_openconnect_path() { return ""; }
std::string get_openconnect_path(const std::string &) { return ""; }
std::string get_bundled_runtime_dir() { return ""; }
std::string get_bundled_wintun_path() { return ""; }
std::string get_bundled_tap_installer_path() { return ""; }

} // namespace platform
} // namespace ecnuvpn

int main() {
  bool ok = true;

  ecnuvpn::Config native_cfg;
  native_cfg.vpn_engine = "native";
  nlohmann::json native_status =
      ecnuvpn::platform::runtime_status_json(
          ecnuvpn::config::to_platform_config_view(native_cfg));
  ok = expect(native_status.value("engine", std::string()) == "native",
              "runtime status should expose native engine mode") &&
       ok;
  ok = expect(native_status.value("available", false),
              "native engine should be available without openconnect") &&
       ok;
  ok = expect(native_status.value("source", std::string()) == "native",
              "native engine source should not be classified as missing") &&
       ok;
  ok = expect(native_status.value("mode", std::string()) == "native",
              "native runtime status should expose native mode") &&
       ok;
  ok = expect(native_status.value("path", std::string()).empty(),
              "native runtime status should not expose a runtime path") &&
       ok;
  ok = expect(!native_status.contains("legacy_openconnect"),
              "native status must not retain legacy OpenConnect diagnostics") &&
       ok;
  ok = expect(!native_status.contains("bundled_path"),
              "native status must not expose OpenConnect bundled_path") &&
       ok;
  ok = expect(!native_status.contains("system_path"),
              "native status must not expose OpenConnect system_path") &&
       ok;

  ecnuvpn::Config legacy_cfg;
  legacy_cfg.vpn_engine = "native";
  nlohmann::json legacy_status =
      ecnuvpn::platform::runtime_status_json(
          ecnuvpn::config::to_platform_config_view(legacy_cfg));
  ok = expect(legacy_status.value("engine", std::string()) == "native",
              "runtime status should remain native-only for all config views") &&
       ok;

  return ok ? 0 : 1;
}
