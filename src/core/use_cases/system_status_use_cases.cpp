#include "core/use_cases/system_status_use_cases.hpp"

#include "core/config/config_platform_view.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"
#include "platform/common/status_models.hpp"

#include <utility>

namespace exv::core {

SystemStatusUseCases::SystemStatusUseCases()
    : SystemStatusUseCases(ecnuvpn::platform::get_config_dir()) {}

SystemStatusUseCases::SystemStatusUseCases(std::string config_dir)
    : manager_(std::move(config_dir)) {
  ecnuvpn::platform::logging::configure_default_logging(false);
}

UseCaseResult SystemStatusUseCases::service_status() {
  return UseCaseResult::ok(ecnuvpn::platform::service_status_to_json(
      ecnuvpn::platform::current_service_status()));
}

UseCaseResult SystemStatusUseCases::helper_status() {
  ecnuvpn::platform::BackendResolveOptions options;
  options.preferred_mode = "auto";
  options.allow_oneshot = true;
  options.allow_service_start = false;
  nlohmann::json resolved = ecnuvpn::platform::resolve_backend(options);
  if (!resolved.value("ok", false)) {
    resolved["resolved"] = false;
    resolved["resolution_code"] = resolved.value("code", std::string());
    resolved["resolution_message"] = resolved.value("message", std::string());
    resolved["ok"] = true;
  } else {
    resolved["resolved"] = true;
  }
  return UseCaseResult::ok(resolved);
}

UseCaseResult SystemStatusUseCases::runtime_status() {
  ecnuvpn::Config cfg = manager_.load();
  return UseCaseResult::ok(ecnuvpn::platform::runtime_status_json(
      ecnuvpn::config::to_platform_config_view(cfg)));
}

UseCaseResult SystemStatusUseCases::driver_status() {
  ecnuvpn::Config cfg = manager_.load();
  return UseCaseResult::ok(ecnuvpn::platform::driver_status_json(
      ecnuvpn::config::to_platform_config_view(cfg)));
}

UseCaseResult
SystemStatusUseCases::install_driver(const nlohmann::json &payload) {
  ecnuvpn::Config cfg = manager_.load();
  nlohmann::json result = ecnuvpn::platform::install_driver(
      ecnuvpn::config::to_platform_config_view(cfg), payload);
  if (result.is_object() && result.value("ok", true) == false) {
    return UseCaseResult::fail(result.value("code", "driver_install_failed"),
                               result.value("error", "Driver install failed"));
  }
  return UseCaseResult::ok(result);
}

UseCaseResult SystemStatusUseCases::install_helper_unsupported() {
  return UseCaseResult::fail(
      "unsupported_action",
      "Helper service installation is not exposed through core RPC.");
}

UseCaseResult SystemStatusUseCases::uninstall_helper_unsupported() {
  return UseCaseResult::fail(
      "unsupported_action",
      "Helper service uninstallation is not exposed through core RPC.");
}

} // namespace exv::core
