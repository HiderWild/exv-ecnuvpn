#include "platform/common/driver_status.hpp"

namespace exv {
namespace platform {

nlohmann::json driver_status_json(const ConfigView &cfg) {
  return nlohmann::json{{"preferred", cfg.windows_tunnel_driver},
                        {"tap_interface", cfg.windows_tap_interface},
                        {"supported", false},
                        {"wintun_missing", true},
                        {"tap_missing", true},
                        {"effective_driver_status", "unavailable"}};
}

nlohmann::json install_driver(const ConfigView &cfg,
                              const nlohmann::json &payload) {
  (void)cfg;
  (void)payload;
  return nlohmann::json{{"ok", false},
                        {"error",
                         "Driver installation is only supported on Windows."}};
}

void set_driver_status_adapter_snapshot_provider_for_testing(
    std::function<WindowsDriverAdapterSnapshot()> /*provider*/) {}

void invalidate_driver_status_cache() {}

void clear_driver_status_cache_for_testing() {
  invalidate_driver_status_cache();
}

} // namespace platform
} // namespace exv
