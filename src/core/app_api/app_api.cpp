#include "core/app_api/app_api.hpp"

#include "core/app_api/desktop_action_registry.hpp"
#include "core/tunnel_controller/tunnel_controller_active.hpp"

namespace exv {
namespace app_api {

nlohmann::json handle_action(const std::string &action,
                             const nlohmann::json &payload) {
  return dispatch_desktop_action(action, payload);
}

bool is_tunnel_controller_active() {
  return exv::core::is_tunnel_controller_active();
}

} // namespace app_api
} // namespace exv
