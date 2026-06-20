#include "core/app_api/desktop_action_registry.hpp"

#include "core/app_api/desktop_config_actions.hpp"
#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_log_actions.hpp"
#include "core/app_api/desktop_route_actions.hpp"
#include "core/app_api/desktop_system_actions.hpp"
#include "core/app_api/desktop_vpn_actions.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"

#include <exception>
#include <mutex>

namespace exv {
namespace app_api {

exv::core_api::DesktopRpcAdapter &desktop_adapter() {
  static exv::core_api::DesktopRpcAdapter adapter;
  static std::once_flag initialized;
  std::call_once(initialized, [] {
    register_desktop_vpn_actions(adapter);
    register_desktop_config_actions(adapter);
    register_desktop_route_actions(adapter);
    register_desktop_system_actions(adapter);
    register_desktop_log_actions(adapter);
  });
  return adapter;
}

nlohmann::json dispatch_desktop_action(const std::string &action,
                                       const nlohmann::json &payload) {
  try {
    return desktop_adapter().dispatch(action, payload);
  } catch (const std::exception &ex) {
    return error(ex.what());
  } catch (...) {
    return error("Unknown desktop API error");
  }
}

// End inlined from core/app_api/app_api_desktop_handlers include-unit
} // namespace app_api
} // namespace exv
