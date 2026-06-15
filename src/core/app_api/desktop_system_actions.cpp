#include "core/app_api/desktop_system_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/app_api/desktop_status_presenter.hpp"
#include "core/app_api/desktop_tunnel_host.hpp"
#include "core/connection/connection_attempt.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/use_cases/system_status_use_cases.hpp"
#include "helper/common/helper_client.hpp"
#include "helper/common/helper_connector.hpp"

#include <utility>

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

template <typename ServiceResponse>
exv::core::UseCaseResult helper_service_result(
    const ServiceResponse &response, const char *error_code,
    const char *fallback_message) {
  auto use_cases = make_system_status_use_cases();
  nlohmann::json payload{
      {"operation",
       {{"success", response.success},
        {"exit_code", response.exit_code},
        {"message", response.message}}},
  };
  auto service_status = use_cases.service_status();
  if (service_status.success) {
    payload["service_status"] = service_status.payload;
  }
  if (response.success) {
    return exv::core::UseCaseResult::ok(std::move(payload));
  }
  return exv::core::UseCaseResult::fail(
      error_code, response.message.empty() ? fallback_message : response.message);
}

bool current_vpn_session_active() {
  auto controller = get_tunnel_controller_if_exists();
  if (!controller) {
    return false;
  }
  const auto snap = controller->status();
  return snap.session_active || snap.network_ready || snap.desired_connected;
}

bool current_helper_is_active_oneshot(const exv::core::TunnelStatusSnapshot &snap) {
  return snap.session_active &&
         (snap.helper_mode == "oneshot" || snap.helper_mode == "transient");
}

exv::core::UseCaseResult helper_status_with_current_instance() {
  auto use_cases = make_system_status_use_cases();
  auto helper = use_cases.helper_status();
  if (!helper.success) {
    return helper;
  }

  auto payload = helper.payload;
  auto service = use_cases.service_status();
  if (service.success) {
    payload["service_status"] = service.payload;
  }

  if (auto controller = get_tunnel_controller_if_exists()) {
    auto snap = controller->status();
    if (snap.helper_status != "unavailable") {
      payload["current_instance"] =
          helper_current_instance_from_controller_snapshot(snap);
    }
  }

  return exv::core::UseCaseResult::ok(std::move(payload));
}

template <typename ServiceResponse>
exv::core::UseCaseResult helper_service_failure_with_operation(
    const ServiceResponse &response, const char *error_code,
    const std::string &message) {
  nlohmann::json payload{
      {"operation",
       {{"success", response.success},
        {"exit_code", response.exit_code},
        {"message", response.message}}},
  };
  return exv::core::UseCaseResult::fail(error_code, message);
}

std::string helper_mode_wire_name(exv::helper::HelperMode mode) {
  switch (mode) {
  case exv::helper::HelperMode::Resident:
    return "resident";
  case exv::helper::HelperMode::Transient:
  default:
    return "oneshot";
  }
}

exv::core::UseCaseResult handoff_current_oneshot_to_service(
    const exv::helper::InstallServiceResponse &install_response,
    const std::shared_ptr<exv::helper::HelperClient> &oneshot_client) {
  if (!oneshot_client || !oneshot_client->is_connected()) {
    return helper_service_failure_with_operation(
        install_response, "handoff_failed",
        "The current helper instance is unavailable for service handoff.");
  }

  auto lease_export =
      oneshot_client->export_cleanup_lease(exv::helper::ExportCleanupLeaseRequest{});
  if (!lease_export.has_active_session || lease_export.lease.sessions.empty()) {
    return helper_service_failure_with_operation(
        install_response, "handoff_failed",
        "The current helper did not export an active cleanup lease.");
  }

  auto service_connector = exv::helper::HelperConnector::create();
  exv::helper::HelperConnectorConfig service_config;
  service_config.mode = exv::helper::ConnectorMode::Resident;
  service_config.connect_timeout_ms = 5000;

  auto service_unique = service_connector->connect(service_config);
  std::shared_ptr<exv::helper::HelperClient> service_client(
      std::move(service_unique));
  if (!service_client || !service_client->is_connected()) {
    return helper_service_failure_with_operation(
        install_response, "handoff_failed",
        "Installed helper service is not reachable for session handoff.");
  }

  auto service_hello = service_client->hello(exv::helper::HelloRequest{});
  if (service_hello.mode != exv::helper::HelperMode::Resident) {
    return helper_service_failure_with_operation(
        install_response, "handoff_wrong_mode",
        "The helper reached after installation is not a service instance.");
  }

  exv::helper::AcquireCoreLeaseRequest acquire;
  acquire.core_pid = ecnuvpn::connection_attempt::current_process_id();
  acquire.purpose = "service.handoff";
  auto service_lease = service_client->acquire_core_lease(acquire);
  if (!service_lease.accepted || service_lease.lease_id.empty()) {
    return helper_service_failure_with_operation(
        install_response, "core_lease_unavailable",
        "Helper service could not acquire a core lease for handoff.");
  }

  auto release_service_lease = [&] {
    exv::helper::ReleaseCoreLeaseRequest release;
    release.lease_id = service_lease.lease_id;
    release.exit_if_oneshot = false;
    (void)service_client->release_core_lease(release);
  };

  exv::helper::HandoffSessionRequest handoff;
  handoff.lease = lease_export.lease;
  auto handoff_response = service_client->handoff_session(handoff);
  if (!handoff_response.adopted) {
    release_service_lease();
    return helper_service_failure_with_operation(
        install_response, "handoff_failed",
        handoff_response.message.empty()
            ? "Helper service rejected the cleanup lease handoff."
            : handoff_response.message);
  }

  const std::string service_mode =
      service_lease.mode.empty() ? helper_mode_wire_name(service_hello.mode)
                                 : service_lease.mode;
  std::string service_endpoint = service_hello.startup_context.endpoint;
  if (service_endpoint.empty()) {
    service_endpoint = service_config.pipe_endpoint;
  }

  if (!replace_tunnel_controller_helper_for_handoff(
          std::move(service_connector), service_client, service_lease.lease_id,
          service_mode, service_endpoint)) {
    release_service_lease();
    return helper_service_failure_with_operation(
        install_response, "handoff_failed",
        "Core could not switch the active tunnel controller to the service helper.");
  }

  auto finalize = oneshot_client->finalize_handoff(
      exv::helper::FinalizeHandoffRequest{true});
  if (!finalize.finalized || !finalize.exiting) {
    return helper_service_failure_with_operation(
        install_response, "handoff_finalize_failed",
        "The one-shot helper did not acknowledge handoff finalization.");
  }

  nlohmann::json payload{
      {"operation",
       {{"success", install_response.success},
        {"exit_code", install_response.exit_code},
        {"message", install_response.message}}},
      {"handoff",
       {{"adopted", handoff_response.adopted},
        {"session_count", handoff_response.session_ids.size()},
        {"service_endpoint", service_endpoint},
        {"oneshot_finalized", finalize.finalized},
        {"oneshot_exiting", finalize.exiting}}}};
  auto service_status = make_system_status_use_cases().service_status();
  if (service_status.success) {
    payload["service_status"] = service_status.payload;
  }
  return exv::core::UseCaseResult::ok(std::move(payload));
}

exv::core::UseCaseResult install_helper_service_with_current_instance() {
  auto controller = get_tunnel_controller_if_exists();
  const bool should_handoff =
      controller && current_helper_is_active_oneshot(controller->status());

  if (auto client = get_current_helper_client_if_exists()) {
    if (client->is_connected()) {
      auto response =
          client->install_service(exv::helper::InstallServiceRequest{});
      if (!response.success || !should_handoff) {
        return helper_service_result(response, "service_install_failed",
                                     "Helper service installation failed.");
      }
      return handoff_current_oneshot_to_service(response, client);
    }
  }
  return make_system_status_use_cases().install_helper();
}

exv::core::UseCaseResult uninstall_helper_service_with_current_instance() {
  if (current_vpn_session_active()) {
    return exv::core::UseCaseResult::fail(
        "vpn_session_active",
        "Disconnect the VPN session before uninstalling the helper service.");
  }
  if (auto client = get_current_helper_client_if_exists()) {
    if (client->is_connected()) {
      return helper_service_result(
          client->uninstall_service(exv::helper::UninstallServiceRequest{}),
          "service_uninstall_failed", "Helper service uninstallation failed.");
    }
  }
  return make_system_status_use_cases().uninstall_helper();
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
        return desktop_result(helper_status_with_current_instance());
      });

  adapter.register_legacy_handler(
      "service.install", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(install_helper_service_with_current_instance());
      });

  adapter.register_legacy_handler(
      "service.uninstall", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return desktop_result(uninstall_helper_service_with_current_instance());
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
