#include "core/app_api/desktop_system_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "core/use_cases/system_status_use_cases.hpp"

namespace ecnuvpn {
namespace app_api {
namespace {

exv::core::SystemStatusUseCases make_system_status_use_cases() {
  return exv::core::SystemStatusUseCases();
}

nlohmann::json desktop_result(const exv::core::UseCaseResult &result) {
  if (!result.success) {
    return error(result.error_message, result.error_code);
  }
  return result.payload;
}

} // namespace

void register_desktop_system_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "service.status", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(make_system_status_use_cases().service_status());
      });

  adapter.register_legacy_handler(
      "helper.status", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(make_system_status_use_cases().helper_status());
      });

  adapter.register_legacy_handler(
      "runtime.status", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(make_system_status_use_cases().runtime_status());
      });

  adapter.register_legacy_handler(
      "drivers.status", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(make_system_status_use_cases().driver_status());
      });

  adapter.register_legacy_handler(
      "drivers.install", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(
            make_system_status_use_cases().install_driver(payload));
      });
}

} // namespace app_api
} // namespace ecnuvpn
