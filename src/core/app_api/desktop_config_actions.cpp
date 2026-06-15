#include "core/app_api/desktop_config_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "core/use_cases/config_use_cases.hpp"

namespace ecnuvpn {
namespace app_api {
namespace {

exv::core::ConfigUseCases make_config_use_cases() {
  return exv::core::ConfigUseCases();
}

nlohmann::json desktop_result(const exv::core::UseCaseResult &result) {
  if (!result.success) {
    return error(result.error_message, result.error_code);
  }
  return result.payload;
}

} // namespace

void register_desktop_config_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "config.getAuth", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(make_config_use_cases().get_auth());
      });

  adapter.register_legacy_handler(
      "config.saveAuth", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(make_config_use_cases().save_auth(payload));
      });

  adapter.register_legacy_handler(
      "config.getSettings",
      [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(make_config_use_cases().get_settings());
      });

  adapter.register_legacy_handler(
      "config.saveSettings",
      [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        const auto result = make_config_use_cases().save_settings(payload);
        if (!result.success) {
          return error(result.error_message, result.error_code);
        }
        return result.payload.value("settings", nlohmann::json::object());
      });

  adapter.register_legacy_handler(
      "config.getKey", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(make_config_use_cases().get_key_status());
      });
}

} // namespace app_api
} // namespace ecnuvpn
