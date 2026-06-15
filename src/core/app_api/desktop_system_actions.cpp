#include "core/app_api/desktop_system_actions.hpp"

#include "core/app_api/desktop_runtime_context.hpp"
#include "core/app_api/desktop_status_presenter.hpp"
#include "core/config/config_manager.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "common/diagnostics/logger.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/helper_client.hpp"
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

void register_desktop_system_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "service.status", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return service_status_json();
      });

  adapter.register_legacy_handler(
      "helper.status", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        platform::BackendResolveOptions options;
        options.preferred_mode = "auto";
        options.allow_oneshot = true;
        options.allow_service_start = false;
        nlohmann::json resolved = platform::resolve_backend(options);
        if (!resolved.value("ok", false)) {
          resolved["resolved"] = false;
          resolved["resolution_code"] =
              resolved.value("code", std::string());
          resolved["resolution_message"] =
              resolved.value("message", std::string());
          resolved["ok"] = true;
        } else {
          resolved["resolved"] = true;
        }
        return resolved;
      });

  adapter.register_legacy_handler(
      "runtime.status", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        return runtime_status_json(cfg);
      });

  adapter.register_legacy_handler(
      "drivers.status", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        return driver_status_json(cfg);
      });

  adapter.register_legacy_handler(
      "drivers.install", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        return install_driver(cfg, payload);
      });
}

} // namespace app_api
} // namespace ecnuvpn
