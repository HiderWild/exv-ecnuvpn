#include "core/config/config.hpp"
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
namespace utils {

std::vector<std::string> split_lines(const std::string &) { return {}; }
std::string run_command_output(const std::string &) { return ""; }
std::string shell_quote(const std::string &value) { return value; }
std::string get_bundled_openconnect_path() { return ""; }
std::string get_openconnect_path(const std::string &) { return ""; }
std::string get_bundled_runtime_dir() { return ""; }
std::string get_bundled_wintun_path() { return ""; }
std::string get_bundled_tap_installer_path() { return ""; }

} // namespace utils
} // namespace ecnuvpn

int main() {
  bool ok = true;

  ecnuvpn::Config native_cfg;
  native_cfg.vpn_engine = "native";
  nlohmann::json native_status =
      ecnuvpn::platform::runtime_status_json(native_cfg);
  ok = expect(native_status.value("engine", std::string()) == "native",
              "runtime status should expose native engine mode") &&
       ok;
  ok = expect(native_status.value("available", false),
              "native engine should be available without openconnect") &&
       ok;
  ok = expect(native_status.value("source", std::string()) == "native",
              "native engine source should not be classified as missing") &&
       ok;
  ok = expect(native_status.contains("legacy_openconnect"),
              "native status should retain legacy OpenConnect diagnostics") &&
       ok;
  ok = expect(!native_status["legacy_openconnect"].value("available", true),
              "legacy diagnostics should still report missing openconnect") &&
       ok;

  ecnuvpn::Config legacy_cfg;
  legacy_cfg.vpn_engine = "legacy_openconnect";
  nlohmann::json legacy_status =
      ecnuvpn::platform::runtime_status_json(legacy_cfg);
  ok = expect(legacy_status.value("engine", std::string()) ==
                  "legacy_openconnect",
              "legacy runtime status should expose legacy engine mode") &&
       ok;
  ok = expect(!legacy_status.value("available", true),
              "legacy OpenConnect mode should still require an executable") &&
       ok;
  ok = expect(legacy_status.value("source", std::string()) == "missing",
              "legacy OpenConnect source should report missing") &&
       ok;

  return ok ? 0 : 1;
}
