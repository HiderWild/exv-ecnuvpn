#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/config_view.hpp"
#include "platform/common/driver_status.hpp"

#include <iostream>

namespace ecnuvpn {
namespace platform {

std::string get_bundled_wintun_path() { return ""; }
std::string get_bundled_tap_installer_path() { return ""; }

} // namespace platform
} // namespace ecnuvpn

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

ecnuvpn::platform::ConfigView base_config() {
  ecnuvpn::platform::ConfigView cfg;
  cfg.vpn_engine = "native";
  cfg.windows_tunnel_driver = "auto";
  cfg.windows_tap_interface = "";
  return cfg;
}

bool driver_status_reuses_one_adapter_snapshot_for_burst() {
  bool ok = true;
  int scans = 0;

  ecnuvpn::platform::clear_driver_status_cache_for_testing();
  ecnuvpn::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      [&] {
        ++scans;
        return ecnuvpn::platform::WindowsDriverAdapterSnapshot{
            {"Wintun Userspace Tunnel"},
            {"ECNU VPN TAP"}};
      });

  const auto cfg = base_config();
  auto first = ecnuvpn::platform::driver_status_json(cfg);
  auto second = ecnuvpn::platform::driver_status_json(cfg);

  ok = expect(scans == 1,
              "driver status burst should reuse a single adapter snapshot") &&
       ok;
  ok = expect(first.value("effective_driver", std::string()) == "tap",
              "auto driver should fall back to TAP when wintun.dll is missing") &&
       ok;
  ok = expect(second.value("wintun_missing", false),
              "existing Wintun adapters must not hide a missing bundled wintun.dll") &&
       ok;
  ok = expect(second.value("effective_driver_status", std::string()) == "ready",
              "cached status should still be ready when TAP is available") &&
       ok;

  ecnuvpn::platform::clear_driver_status_cache_for_testing();
  (void)ecnuvpn::platform::driver_status_json(cfg);
  ok = expect(scans == 2,
              "clearing driver status cache should force a fresh scan") &&
       ok;

  ecnuvpn::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      nullptr);
  ecnuvpn::platform::clear_driver_status_cache_for_testing();
  return ok;
}

bool wintun_adapter_without_runtime_is_unavailable() {
  bool ok = true;

  ecnuvpn::platform::clear_driver_status_cache_for_testing();
  ecnuvpn::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      [] {
        return ecnuvpn::platform::WindowsDriverAdapterSnapshot{
            {"Wintun Userspace Tunnel"},
            {}};
      });

  const auto cfg = base_config();
  auto status = ecnuvpn::platform::driver_status_json(cfg);

  ok = expect(status.value("effective_driver", std::string()) == "wintun",
              "auto driver should still report the intended Wintun driver") &&
       ok;
  ok = expect(status.value("wintun_missing", false),
              "Wintun should be missing when wintun.dll is not bundled") &&
       ok;
  ok = expect(status.value("effective_driver_status", std::string()) ==
                  "unavailable",
              "Wintun without runtime DLL should be unavailable") &&
       ok;

  ecnuvpn::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      nullptr);
  ecnuvpn::platform::clear_driver_status_cache_for_testing();
  return ok;
}

bool preflight_reuses_cached_driver_snapshot() {
  bool ok = true;
  int scans = 0;

  ecnuvpn::platform::clear_driver_status_cache_for_testing();
  ecnuvpn::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      [&] {
        ++scans;
        return ecnuvpn::platform::WindowsDriverAdapterSnapshot{
            {},
            {"ECNU VPN TAP"}};
      });

  auto cfg = base_config();
  cfg.windows_tunnel_driver = "tap";
  cfg.windows_tap_interface = "ECNU VPN TAP";

  auto first = ecnuvpn::platform::preflight_connect_platform_checks(cfg);
  auto second = ecnuvpn::platform::preflight_connect_platform_checks(cfg);

  ok = expect(scans == 1,
              "preflight burst should not rescan adapters for each call") &&
       ok;
  ok = expect(!first.is_object() || first.value("ok", true),
              "first preflight should pass with injected TAP adapter") &&
       ok;
  ok = expect(!second.is_object() || second.value("ok", true),
              "second preflight should pass with cached TAP adapter") &&
       ok;

  ecnuvpn::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      nullptr);
  ecnuvpn::platform::clear_driver_status_cache_for_testing();
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = driver_status_reuses_one_adapter_snapshot_for_burst() && ok;
  ok = wintun_adapter_without_runtime_is_unavailable() && ok;
  ok = preflight_reuses_cached_driver_snapshot() && ok;
  return ok ? 0 : 1;
}
