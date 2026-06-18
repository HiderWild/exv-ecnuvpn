#include "platform/common/app_api_runtime_policy.hpp"

#include "platform/common/driver_status.hpp"

#include <cstdlib>
#include <cstdio>
#include <sstream>

namespace ecnuvpn {
namespace platform {
namespace {

struct LocalRuntimeSnapshot {
  bool running = false;
  bool network_ready = false;
  std::string interface_name;
  std::string internal_ip;
};

LocalRuntimeSnapshot read_local_runtime_snapshot() {
  LocalRuntimeSnapshot snapshot;
  return snapshot;
}

void clear_local_runtime_state() {
  // PID/route-ready files removed — no local state to clear
}

} // namespace

void prepare_direct_fallback_runtime() {}

std::string helper_unavailable_connect_message() {
  return "Helper daemon is not available. Install the helper service from Settings or run 'exv service install' as Administrator.";
}

std::string helper_unavailable_disconnect_message() {
  return "Helper daemon is not available. Use the elevated desktop action or install the helper service from Settings.";
}

nlohmann::json preflight_connect_platform_checks(const ConfigView &cfg) {
  nlohmann::json drivers = driver_status_json(cfg);
  std::string effective = drivers.value("effective_driver", std::string("wintun"));
  bool wintun_missing = drivers.value(
      "wintun_missing", !drivers.value("wintun_bundled", false));
  if (cfg.windows_tunnel_driver == "wintun" &&
      wintun_missing) {
    return nlohmann::json{{"ok", false},
                          {"error", "Wintun is selected but no bundled wintun.dll or existing Wintun adapter was detected."}};
  }
  if (effective == "tap" && cfg.windows_tunnel_driver == "tap" &&
      cfg.windows_tap_interface.empty()) {
    return nlohmann::json{{"ok", false},
                          {"error", "TAP is selected but no TAP interface is configured. Choose an installed TAP adapter or switch back to Wintun."}};
  }
  return nlohmann::json{};
}

nlohmann::json try_connect_direct_fallback(const ConfigView & /*cfg*/,
                                            const std::string & /*password*/) {
  return nlohmann::json{};
}

nlohmann::json try_disconnect_direct_fallback(bool allow_direct_fallback) {
  if (!allow_direct_fallback)
    return nlohmann::json{};

  LocalRuntimeSnapshot snapshot = read_local_runtime_snapshot();
  if (!snapshot.running)
    return nlohmann::json{{"ok", true}, {"_not_running", true}};

  clear_local_runtime_state();
  return nlohmann::json{{"ok", true}, {"_direct_fallback", true}};
}

nlohmann::json status_fallback_without_helper(const ConfigView & /*cfg*/) {
  LocalRuntimeSnapshot snapshot = read_local_runtime_snapshot();
  nlohmann::json result;
  result["_snapshot"] = true;
  result["_running"] = snapshot.running;
  result["_snapshot_data"] = nlohmann::json{
      {"running", snapshot.running},
      {"network_ready", snapshot.network_ready},
      {"interface", snapshot.interface_name},
      {"internal_ip", snapshot.internal_ip},
  };
  return result;
}

} // namespace platform
} // namespace ecnuvpn
