#include "core/app_api/desktop_route_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/app_api/desktop_status_presenter.hpp"
#include "core/config/config_api.hpp"
#include "core/config/config_manager.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "common/diagnostics/logger.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/runtime_paths.hpp"

namespace ecnuvpn {
namespace app_api {
namespace {

config::ConfigManager make_config_manager() {
  platform::ensure_dir(platform::get_config_dir());
  logger::init();
  return config::ConfigManager(platform::get_config_dir());
}

} // namespace

void register_desktop_route_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "routes.list", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        return routes_json(cfg);
      });

  adapter.register_legacy_handler(
      "routes.add", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        std::string err = config_api::route_add(mgr, payload.value("cidr", ""));
        if (!err.empty()) {
          return error(err);
        }
        return routes_json(mgr.load());
      });

  adapter.register_legacy_handler(
      "routes.remove", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        std::string err =
            config_api::route_remove(mgr, payload.value("cidr", ""));
        if (!err.empty()) {
          return error(err);
        }
        return routes_json(mgr.load());
      });

  adapter.register_legacy_handler(
      "routes.reset", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        config_api::route_reset_defaults(mgr);
        return routes_json(mgr.load());
      });
}

} // namespace app_api
} // namespace ecnuvpn
