#include "platform/common/app_api_runtime_policy.hpp"

#include "platform/common/driver_status.hpp"

namespace ecnuvpn {
namespace platform {

void prepare_direct_fallback_runtime() {}

std::string helper_unavailable_connect_message() {
  return "Helper daemon is not available. Install the helper service from Settings or run 'exv service install' as Administrator.";
}

std::string helper_unavailable_disconnect_message() {
  return "Helper daemon is not available. Use the elevated desktop action or install the helper service from Settings.";
}

nlohmann::json preflight_connect_platform_checks(const Config &cfg) {
  nlohmann::json drivers = driver_status_json(cfg);
  std::string effective = drivers.value("effective_driver", std::string("wintun"));
  if (cfg.windows_tunnel_driver == "wintun" &&
      !drivers.value("wintun_bundled", false)) {
    return nlohmann::json{{"ok", false},
                          {"error", "Wintun is selected but bundled wintun.dll is missing."}};
  }
  if (effective == "tap" && cfg.windows_tunnel_driver == "tap" &&
      cfg.windows_tap_interface.empty()) {
    return nlohmann::json{{"ok", false},
                          {"error", "TAP is selected but no TAP interface is configured. Choose an installed TAP adapter or switch back to Wintun."}};
  }
  return nlohmann::json{};
}

nlohmann::json try_connect_direct_fallback(const Config & /*cfg*/,
                                            const std::string & /*password*/) {
  // Windows always requires the helper service; no direct fallback.
  return nlohmann::json{};
}

nlohmann::json try_disconnect_direct_fallback(bool /*allow_direct_fallback*/) {
  // Windows always requires the helper service; no direct fallback.
  return nlohmann::json{};
}

nlohmann::json status_fallback_without_helper(const Config & /*cfg*/) {
  // Windows always expects the helper; no snapshot fallback.
  return nlohmann::json{};
}

} // namespace platform
} // namespace ecnuvpn