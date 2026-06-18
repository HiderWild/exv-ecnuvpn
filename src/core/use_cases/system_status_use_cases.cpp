#include "core/use_cases/system_status_use_cases.hpp"

#include "core/config/config_platform_view.hpp"
#include "core/connection/connection_attempt.hpp"
#include "core/vpn/vpn.hpp"
#include "helper/common/helper_client.hpp"
#include "helper/common/helper_connector.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"
#include "platform/common/status_models.hpp"

#include <filesystem>
#include <utility>

namespace exv::core {
namespace {

template <typename ServiceResponse>
UseCaseResult service_op_result(const ServiceResponse &response,
                                const char *error_code,
                                const char *fallback_message) {
  nlohmann::json payload{{"operation",
                          {{"success", response.success},
                           {"exit_code", response.exit_code},
                           {"message", response.message}}},
                         {"service_status",
                          ecnuvpn::platform::service_status_to_json(
                              ecnuvpn::platform::current_service_status())}};
  if (response.success) {
    return UseCaseResult::ok(std::move(payload));
  }
  return UseCaseResult::fail(
      error_code, response.message.empty() ? fallback_message : response.message);
}

template <typename Fn>
UseCaseResult with_helper_service_lease(const std::string &purpose,
                                        bool bootstrap_oneshot,
                                        const std::string &preferred_mode,
                                        Fn &&fn) {
  ecnuvpn::platform::BackendResolveOptions options;
  options.preferred_mode = preferred_mode;
  options.allow_oneshot = bootstrap_oneshot;
  options.allow_service_start = false;
  if (bootstrap_oneshot) {
    options.start_oneshot = true;
    const auto exv_path =
        std::filesystem::path(ecnuvpn::platform::get_executable_path());
#ifdef _WIN32
    options.helper_path = (exv_path.parent_path() / "exv-helper.exe").string();
#else
    options.helper_path = (exv_path.parent_path() / "exv-helper").string();
#endif
  }

  nlohmann::json backend = ecnuvpn::platform::resolve_backend(options);
  if (!backend.value("ok", false)) {
    return UseCaseResult::fail(
        backend.value("code", std::string("helper_unavailable")),
        backend.value(
            "message",
            std::string(
                "No helper instance is available for privileged service maintenance.")));
  }

  auto connector = exv::helper::HelperConnector::create();
  exv::helper::HelperConnectorConfig config;
  const std::string backend_mode = backend.value("backend", std::string());
  config.mode = backend_mode == "oneshot"
                    ? exv::helper::ConnectorMode::Transient
                    : exv::helper::ConnectorMode::Resident;
  config.pipe_endpoint = backend.value("endpoint", std::string());
  config.connect_timeout_ms = 500;

  auto client = connector->connect(config);
  if (!client || !client->is_connected()) {
    return UseCaseResult::fail(
        "helper_unavailable",
        "No helper instance is available for privileged service maintenance.");
  }

  (void)client->hello(exv::helper::HelloRequest{});

  exv::helper::AcquireCoreLeaseRequest acquire;
  acquire.core_pid = ecnuvpn::connection_attempt::current_process_id();
  acquire.purpose = purpose;
  auto lease = client->acquire_core_lease(acquire);
  if (!lease.accepted || lease.lease_id.empty()) {
    return UseCaseResult::fail(
        "core_lease_unavailable",
        "Helper could not acquire a core lease for service maintenance.");
  }

  auto release = [&] {
    exv::helper::ReleaseCoreLeaseRequest release_req;
    release_req.lease_id = lease.lease_id;
    release_req.exit_if_oneshot = backend_mode == "oneshot";
    (void)client->release_core_lease(release_req);
  };

  UseCaseResult result = fn(*client);
  release();
  return result;
}

} // namespace

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

UseCaseResult SystemStatusUseCases::install_helper() {
  return with_helper_service_lease("service.install", true, "auto", [](auto &client) {
    return service_op_result(
        client.install_service(exv::helper::InstallServiceRequest{}),
        "service_install_failed", "Helper service installation failed.");
  });
}

UseCaseResult SystemStatusUseCases::uninstall_helper() {
  ecnuvpn::Config cfg = manager_.load();
  auto runtime = ecnuvpn::vpn::read_runtime_status_snapshot(cfg);
  if (runtime.running || runtime.network_ready) {
    return UseCaseResult::fail(
        "vpn_session_active",
        "Disconnect the VPN session before uninstalling the helper service.");
  }
  return with_helper_service_lease("service.uninstall", true, "oneshot", [](auto &client) {
    return service_op_result(
        client.uninstall_service(exv::helper::UninstallServiceRequest{}),
        "service_uninstall_failed", "Helper service uninstallation failed.");
  });
}

} // namespace exv::core
