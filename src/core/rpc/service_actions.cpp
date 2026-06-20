#include "service_actions.hpp"

#include "core/connection/connection_attempt.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "helper/common/helper_client.hpp"
#include "helper/common/helper_connector.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/helper_delegating_network_ops.hpp"

#include <nlohmann/json.hpp>

#include <exception>
#include <memory>
#include <utility>

using json = nlohmann::json;

namespace exv::core_api {
namespace {

RpcResponse to_rpc_response(const exv::core::UseCaseResult &result) {
    RpcResponse resp;
    resp.success = result.success;
    if (result.success) {
        resp.payload_json = result.payload.dump();
    } else {
        resp.error_code = result.error_code;
        resp.error_message = result.error_message;
    }
    return resp;
}

RpcResponse invalid_payload_response(const std::exception &e) {
    return to_rpc_response(
        exv::core::UseCaseResult::fail("invalid_payload", e.what()));
}

bool active_vpn_session(const std::shared_ptr<exv::core::TunnelController> &controller) {
    if (!controller) {
        return false;
    }
    auto snap = controller->status();
    return snap.session_active || snap.network_ready || snap.desired_connected;
}

bool active_oneshot_session(const exv::core::TunnelStatusSnapshot &snap) {
    return snap.session_active &&
           (snap.helper_mode == "oneshot" || snap.helper_mode == "transient");
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

template <typename ServiceResponse>
exv::core::UseCaseResult service_failure(const ServiceResponse &response,
                                         const char *code,
                                         const std::string &message) {
    (void)response;
    return exv::core::UseCaseResult::fail(code, message);
}

exv::core::UseCaseResult install_with_active_controller_handoff(
    const std::shared_ptr<exv::core::TunnelController> &controller,
    const exv::helper::InstallServiceResponse &install_response) {
    auto oneshot_client = controller ? controller->helper_client_for_maintenance()
                                     : nullptr;
    if (!oneshot_client || !oneshot_client->is_connected()) {
        return service_failure(
            install_response, "handoff_failed",
            "The current helper instance is unavailable for service handoff.");
    }

    auto lease_export =
        oneshot_client->export_cleanup_lease(exv::helper::ExportCleanupLeaseRequest{});
    if (!lease_export.has_active_session || lease_export.lease.sessions.empty()) {
        return service_failure(
            install_response, "handoff_failed",
            "The current helper did not export an active cleanup lease.");
    }

    auto connector = exv::helper::HelperConnector::create();
    exv::helper::HelperConnectorConfig config;
    config.mode = exv::helper::ConnectorMode::Resident;
    config.connect_timeout_ms = 5000;

    auto unique_client = connector->connect(config);
    std::shared_ptr<exv::helper::HelperClient> service_client(
        std::move(unique_client));
    if (!service_client || !service_client->is_connected()) {
        return service_failure(
            install_response, "handoff_failed",
            "Installed helper service is not reachable for session handoff.");
    }

    auto hello = service_client->hello(exv::helper::HelloRequest{});
    if (hello.mode != exv::helper::HelperMode::Resident) {
        return service_failure(
            install_response, "handoff_wrong_mode",
            "The helper reached after installation is not a service instance.");
    }

    exv::helper::AcquireCoreLeaseRequest acquire;
    acquire.core_pid = exv::connection_attempt::current_process_id();
    acquire.purpose = "service.handoff";
    auto service_lease = service_client->acquire_core_lease(acquire);
    if (!service_lease.accepted || service_lease.lease_id.empty()) {
        return service_failure(
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
        return service_failure(
            install_response, "handoff_failed",
            handoff_response.message.empty()
                ? "Helper service rejected the cleanup lease handoff."
                : handoff_response.message);
    }

    auto service_ops =
        std::make_shared<exv::platform::HelperDelegatingPlatformNetworkOps>(
            service_client.get());
    const std::string service_mode =
        service_lease.mode.empty() ? helper_mode_wire_name(hello.mode)
                                   : service_lease.mode;
    const std::string endpoint = hello.startup_context.endpoint;
    if (!controller->replace_helper_for_handoff(
            service_client, service_ops, service_lease.lease_id, service_mode,
            endpoint)) {
        release_service_lease();
        return service_failure(
            install_response, "handoff_failed",
            "Core could not switch the active tunnel controller to the service helper.");
    }

    auto finalize = oneshot_client->finalize_handoff(
        exv::helper::FinalizeHandoffRequest{true});
    if (!finalize.finalized || !finalize.exiting) {
        return service_failure(
            install_response, "handoff_finalize_failed",
            "The one-shot helper did not acknowledge handoff finalization.");
    }

    return exv::core::UseCaseResult::ok(json{
        {"operation",
         {{"success", install_response.success},
          {"exit_code", install_response.exit_code},
          {"message", install_response.message}}},
        {"handoff",
         {{"adopted", handoff_response.adopted},
          {"session_count", handoff_response.session_ids.size()},
          {"service_endpoint", endpoint},
          {"oneshot_finalized", finalize.finalized},
          {"oneshot_exiting", finalize.exiting}}}});
}

} // namespace

ServiceActions::ServiceActions() = default;

ServiceActions::ServiceActions(
    std::shared_ptr<exv::core::TunnelController> controller)
    : controller_(std::move(controller)) {}

ServiceActions::ServiceActions(std::string config_dir)
    : use_cases_(std::move(config_dir)) {}

ServiceActions::ServiceActions(
    std::string config_dir,
    std::shared_ptr<exv::core::TunnelController> controller)
    : use_cases_(std::move(config_dir)), controller_(std::move(controller)) {}

void ServiceActions::register_handlers(AppRpcDispatcher& dispatcher) {
    dispatcher.register_handler("service.helper_status",
        [this](const RpcRequest& req) { return helper_status(req); });
    dispatcher.register_handler("service.install",
        [this](const RpcRequest& req) { return install_helper(req); });
    dispatcher.register_handler("service.uninstall",
        [this](const RpcRequest& req) { return uninstall_helper(req); });
    dispatcher.register_handler("service.driver_status",
        [this](const RpcRequest& req) { return driver_status(req); });

    // Desktop API names (match webui/desktop/shared/desktop-contract.ts)
    dispatcher.register_handler("service.status",
        [this](const RpcRequest& req) { return helper_status(req); });
    dispatcher.register_handler("helper.status",
        [this](const RpcRequest& req) { return helper_status(req); });
    dispatcher.register_handler("drivers.status",
        [this](const RpcRequest& req) { return driver_status(req); });
    dispatcher.register_handler("drivers.install",
        [this](const RpcRequest& req) { return install_driver(req); });
}

RpcResponse ServiceActions::helper_status(const RpcRequest& req) {
    if (req.action == "service.status") {
        return to_rpc_response(use_cases_.service_status());
    }
    return to_rpc_response(use_cases_.helper_status());
}

RpcResponse ServiceActions::install_helper(const RpcRequest& req) {
    (void)req;
    if (controller_) {
        auto snap = controller_->status();
        if (active_oneshot_session(snap)) {
            auto client = controller_->helper_client_for_maintenance();
            if (client && client->is_connected()) {
                auto install =
                    client->install_service(exv::helper::InstallServiceRequest{});
                if (!install.success) {
                    return to_rpc_response(exv::core::UseCaseResult::fail(
                        "service_install_failed",
                        install.message.empty()
                            ? "Helper service installation failed."
                            : install.message));
                }
                return to_rpc_response(
                    install_with_active_controller_handoff(controller_, install));
            }
        }
    }
    return to_rpc_response(use_cases_.install_helper());
}

RpcResponse ServiceActions::uninstall_helper(const RpcRequest& req) {
    (void)req;
    if (active_vpn_session(controller_)) {
        return to_rpc_response(exv::core::UseCaseResult::fail(
            "vpn_session_active",
            "Disconnect the VPN session before uninstalling the helper service."));
    }
    exv::platform::BackendResolveOptions options;
    options.preferred_mode = "service";
    options.allow_oneshot = false;
    options.allow_service_start = false;
    auto backend = exv::platform::resolve_backend(options);
    if (!backend.value("ok", false)) {
        return to_rpc_response(exv::core::UseCaseResult::fail(
            backend.value("code", std::string("helper_unavailable")),
            backend.value(
                "message",
                std::string("No helper instance is available for privileged service maintenance."))));
    }
    return to_rpc_response(use_cases_.uninstall_helper());
}

RpcResponse ServiceActions::driver_status(const RpcRequest& req) {
    return to_rpc_response(use_cases_.driver_status());
}

RpcResponse ServiceActions::install_driver(const RpcRequest& req) {
    try {
        auto payload = json::parse(req.payload_json);
        return to_rpc_response(use_cases_.install_driver(payload));
    } catch (const std::exception& e) {
        return invalid_payload_response(e);
    }
}

} // namespace exv::core_api
