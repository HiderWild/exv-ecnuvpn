#include "platform/common/driver_status.hpp"

namespace ecnuvpn {
namespace platform {

nlohmann::json driver_status_json(const Config &cfg) {
  return nlohmann::json{{"preferred", cfg.windows_tunnel_driver},
                        {"tap_interface", cfg.windows_tap_interface},
                        {"supported", false}};
}

nlohmann::json install_driver(const Config &cfg,
                              const nlohmann::json &payload) {
  (void)cfg;
  (void)payload;
  return nlohmann::json{{"ok", false},
                        {"error",
                         "Driver installation is only supported on Windows."}};
}

} // namespace platform
} // namespace ecnuvpn
