#include "core/app_api/desktop_route_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "core/use_cases/config_use_cases.hpp"

namespace exv {
namespace app_api {
namespace {

exv::core::ConfigUseCases make_config_use_cases() {
  return exv::core::ConfigUseCases();
}

nlohmann::json desktop_routes_result(const exv::core::UseCaseResult &result) {
  if (!result.success) {
    return error(result.error_message, result.error_code);
  }
  return result.payload.value("routes", nlohmann::json::array());
}

} // namespace

void register_desktop_route_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "routes.list", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_routes_result(make_config_use_cases().list_routes());
      });

  adapter.register_legacy_handler(
      "routes.add", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_routes_result(make_config_use_cases().add_route(payload));
      });

  adapter.register_legacy_handler(
      "routes.remove", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_routes_result(
            make_config_use_cases().remove_route(payload));
      });

  adapter.register_legacy_handler(
      "routes.reset", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_routes_result(make_config_use_cases().reset_routes());
      });
}

} // namespace app_api
} // namespace exv
