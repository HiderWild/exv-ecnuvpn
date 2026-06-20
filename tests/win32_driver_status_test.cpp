#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/config_view.hpp"
#include "platform/common/driver_status.hpp"

#include <iostream>

namespace exv {
namespace platform {

std::string get_bundled_wintun_path() { return ""; }
std::string get_bundled_tap_installer_path() { return ""; }

} // namespace platform
} // namespace exv

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

exv::platform::ConfigView base_config() {
  exv::platform::ConfigView cfg;
  cfg.vpn_engine = "native";
  cfg.windows_tunnel_driver = "auto";
  cfg.windows_tap_interface = "";
  return cfg;
}

bool driver_status_reuses_one_adapter_snapshot_for_burst() {
  bool ok = true;
  int scans = 0;

  exv::platform::clear_driver_status_cache_for_testing();
  exv::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      [&] {
        ++scans;
        return exv::platform::WindowsDriverAdapterSnapshot{
            {"Wintun Userspace Tunnel"},
            {"EXV TAP"}};
      });

  const auto cfg = base_config();
  auto first = exv::platform::driver_status_json(cfg);
  auto second = exv::platform::driver_status_json(cfg);

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

  exv::platform::clear_driver_status_cache_for_testing();
  (void)exv::platform::driver_status_json(cfg);
  ok = expect(scans == 2,
              "clearing driver status cache should force a fresh scan") &&
       ok;

  exv::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      nullptr);
  exv::platform::clear_driver_status_cache_for_testing();
  return ok;
}

bool wintun_adapter_without_runtime_is_unavailable() {
  bool ok = true;

  exv::platform::clear_driver_status_cache_for_testing();
  exv::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      [] {
        return exv::platform::WindowsDriverAdapterSnapshot{
            {"Wintun Userspace Tunnel"},
            {}};
      });

  const auto cfg = base_config();
  auto status = exv::platform::driver_status_json(cfg);

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

  exv::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      nullptr);
  exv::platform::clear_driver_status_cache_for_testing();
  return ok;
}

bool preflight_reuses_cached_driver_snapshot() {
  bool ok = true;
  int scans = 0;

  exv::platform::clear_driver_status_cache_for_testing();
  exv::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      [&] {
        ++scans;
        return exv::platform::WindowsDriverAdapterSnapshot{
            {},
            {"EXV TAP"}};
      });

  auto cfg = base_config();
  cfg.windows_tunnel_driver = "tap";
  cfg.windows_tap_interface = "EXV TAP";

  auto first = exv::platform::preflight_connect_platform_checks(cfg);
  auto second = exv::platform::preflight_connect_platform_checks(cfg);

  ok = expect(scans == 1,
              "preflight burst should not rescan adapters for each call") &&
       ok;
  ok = expect(!first.is_object() || first.value("ok", true),
              "first preflight should pass with injected TAP adapter") &&
       ok;
  ok = expect(!second.is_object() || second.value("ok", true),
              "second preflight should pass with cached TAP adapter") &&
       ok;

  exv::platform::set_driver_status_adapter_snapshot_provider_for_testing(
      nullptr);
  exv::platform::clear_driver_status_cache_for_testing();
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
