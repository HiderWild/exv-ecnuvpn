#include "helper/helper_handler.hpp"
#include "observability/log_facade.hpp"

#include <nlohmann/json.hpp>
#include <sstream>
#include <utility>

namespace exv::helper {

HelperHandler::HelperHandler(HelperLifecyclePolicy policy)
    : HelperHandler(std::move(policy), nullptr) {
}

HelperHandler::HelperHandler(HelperLifecyclePolicy policy,
                             std::shared_ptr<HelperNetworkOps> network_ops)
    : policy_(std::move(policy)), network_ops_(std::move(network_ops)) {
    register_handlers();
}

void HelperHandler::register_handlers() {
    dispatcher_.register_handler(HelperOp::Hello,
        [this](const HelperRequest& req) { return handle_hello(req); });
    dispatcher_.register_handler(HelperOp::StartSession,
        [this](const HelperRequest& req) { return handle_start_session(req); });
    dispatcher_.register_handler(HelperOp::PrepareTunnelDevice,
        [this](const HelperRequest& req) { return handle_prepare_tunnel_device(req); });
    dispatcher_.register_handler(HelperOp::ApplyTunnelConfig,
        [this](const HelperRequest& req) { return handle_apply_tunnel_config(req); });
    dispatcher_.register_handler(HelperOp::Heartbeat,
        [this](const HelperRequest& req) { return handle_heartbeat(req); });
    dispatcher_.register_handler(HelperOp::Cleanup,
        [this](const HelperRequest& req) { return handle_cleanup(req); });
    dispatcher_.register_handler(HelperOp::GetSnapshot,
        [this](const HelperRequest& req) { return handle_get_snapshot(req); });
    dispatcher_.register_handler(HelperOp::Shutdown,
        [this](const HelperRequest& req) { return handle_shutdown(req); });
}

HelperResponse HelperHandler::handle(const HelperRequest& request) {
    // Validate the request before dispatching
    auto validation_error = validator_.validate(request);
    if (validation_error.has_value()) {
        HelperResponse resp;
        resp.op = request.op;
        resp.success = false;
        resp.error_code = std::to_string(static_cast<int>(validation_error->code));
        resp.error_message = validation_error->message;
        return resp;
    }

    return dispatcher_.dispatch(request);
}

void HelperHandler::tick() {
    auto now = std::chrono::steady_clock::now();
    auto active_ids = leases_.active_session_ids();
    for (const auto& id : active_ids) {
        auto lease = leases_.get_session(id);
        if (lease.has_value() && policy_.should_cleanup_stale(*lease, now)) {
            exv::observability::LogFacade::info("[helper] Heartbeat timed out; cleaning session: "
                                  + id.value);
            CleanupPolicy full_policy;
            full_policy.remove_routes = true;
            full_policy.remove_dns = true;
            full_policy.remove_adapter = true;
            full_policy.remove_firewall_rules = true;
            CleanupResponse cleanup_resp = cleanup_session(id, full_policy);
            if (cleanup_resp.success &&
                startup_context_.launch_mode == "oneshot") {
                shutdown_requested_ = true;
            }
        }
    }
}

SessionLeaseManager& HelperHandler::lease_manager() {
    return leases_;
}

CleanupRegistry& HelperHandler::cleanup_registry() {
    return cleanup_;
}

bool HelperHandler::should_stop() const {
    return shutdown_requested_;
}

void HelperHandler::set_startup_context(HelperStartupContext context) {
    startup_context_ = std::move(context);
}

CleanupResponse HelperHandler::cleanup_all_sessions(const CleanupPolicy& policy) {
    CleanupResponse aggregate;
    aggregate.success = true;
    auto active_ids = leases_.active_session_ids();
    for (const auto& id : active_ids) {
        CleanupResponse cleanup_resp = cleanup_session(id, policy);
        if (!cleanup_resp.success) {
            aggregate.success = false;
            aggregate.errors.insert(aggregate.errors.end(),
                                    cleanup_resp.errors.begin(),
                                    cleanup_resp.errors.end());
        }
    }
    return aggregate;
}

// --- Handler implementations ---

HelperResponse HelperHandler::handle_hello(const HelperRequest& req) {
    HelloResponse hello;
    hello.capabilities = {
        "session", "heartbeat", "cleanup", "snapshot", "shutdown"
    };
    hello.mode = (startup_context_.launch_mode == "service" ||
                  startup_context_.launch_mode == "resident")
                     ? HelperMode::Resident
                     : HelperMode::Transient;
    hello.startup_context = startup_context_;
    hello.session_state.active = leases_.active_session_count() > 0;

    nlohmann::json payload;
    to_json(payload, hello);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperHandler::handle_start_session(const HelperRequest& req) {
    StartSessionRequest start_req;
    try {
        auto j = nlohmann::json::parse(req.payload_json);
        start_req = start_session_request_from_json(j);
    } catch (const std::exception& e) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = std::string("Failed to parse StartSession request: ") + e.what();
        return resp;
    }

    if (leases_.active_session_count() > 0) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "session_conflict";
        resp.error_message = "An active helper session already exists";
        return resp;
    }

    CleanupPolicy default_policy;
    SessionId session_id = leases_.create_session(
        start_req.profile_id, start_req.mode, default_policy);

    // Register a cleanup record for the new session
    CleanupRecord record;
    record.session_id = session_id;
    record.created_at = std::chrono::system_clock::now();
    cleanup_.register_session(record);

    exv::observability::LogFacade::info("[helper] Session started: " + session_id.value);

    StartSessionResponse start_resp;
    start_resp.session_id = session_id;

    nlohmann::json payload;
    to_json(payload, start_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperHandler::handle_prepare_tunnel_device(const HelperRequest& req) {
    PrepareTunnelDeviceRequest device_req;
    try {
        auto j = nlohmann::json::parse(req.payload_json);
        device_req = prepare_tunnel_device_request_from_json(j);
    } catch (const std::exception& e) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = std::string("Failed to parse PrepareTunnelDevice request: ") + e.what();
        return resp;
    }

    if (!leases_.has_session(device_req.session_id)) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_session";
        resp.error_message = "Session not found: " + device_req.session_id.value;
        return resp;
    }

    if (!network_ops_) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "network_ops_unavailable";
        resp.error_message = "Helper network operations are not available";
        return resp;
    }

    std::vector<ManagedResource> created_resources;
    PrepareTunnelDeviceResponse device_resp =
        network_ops_->prepare_tunnel_device(device_req, &created_resources);
    for (const auto& resource : created_resources) {
        cleanup_.add_resource(device_req.session_id, resource);
    }

    nlohmann::json payload;
    to_json(payload, device_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = !device_resp.device_path.empty();
    if (!resp.success) {
        resp.error_code = "device_not_found";
        resp.error_message = "Helper network operations did not return a device";
    }
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperHandler::handle_apply_tunnel_config(const HelperRequest& req) {
    ApplyTunnelConfigRequest config_req;
    try {
        auto j = nlohmann::json::parse(req.payload_json);
        config_req = apply_tunnel_config_request_from_json(j);
    } catch (const std::exception& e) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = std::string("Failed to parse ApplyTunnelConfig request: ") + e.what();
        return resp;
    }

    if (!leases_.has_session(config_req.config.session_id)) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_session";
        resp.error_message = "Session not found: " + config_req.config.session_id.value;
        return resp;
    }

    if (!network_ops_) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "network_ops_unavailable";
        resp.error_message = "Helper network operations are not available";
        return resp;
    }

    std::vector<ManagedResource> created_resources;
    ApplyTunnelConfigResponse config_resp =
        network_ops_->apply_tunnel_config(config_req, &created_resources);
    if (config_resp.success) {
        for (const auto& resource : created_resources) {
            cleanup_.add_resource(config_req.config.session_id, resource);
        }
    }

    nlohmann::json payload;
    to_json(payload, config_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = config_resp.success;
    if (!resp.success) {
        resp.error_code = "network_ops_failed";
        resp.error_message = config_resp.error_message.empty()
                                 ? "Helper network operations failed"
                                 : config_resp.error_message;
    }
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperHandler::handle_heartbeat(const HelperRequest& req) {
    HeartbeatRequest hb_req;
    try {
        auto j = nlohmann::json::parse(req.payload_json);
        hb_req = heartbeat_request_from_json(j);
    } catch (const std::exception& e) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = std::string("Failed to parse Heartbeat request: ") + e.what();
        return resp;
    }

    if (!leases_.has_session(hb_req.session_id)) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_session";
        resp.error_message = "Session not found: " + hb_req.session_id.value;
        return resp;
    }

    leases_.update_heartbeat(hb_req.session_id, hb_req.core_phase);

    // Check if heartbeat indicates a concerning phase
    std::optional<std::string> warning;
    if (hb_req.core_phase == "error" || hb_req.core_phase == "disconnecting") {
        warning = "Core reported phase: " + hb_req.core_phase;
    }

    HeartbeatResponse hb_resp;
    hb_resp.ok = true;
    hb_resp.warning = warning;

    nlohmann::json payload;
    to_json(payload, hb_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

CleanupResponse HelperHandler::cleanup_session(const SessionId& session_id,
                                               const CleanupPolicy& policy) {
    auto resources = cleanup_.get_resources(session_id);
    std::vector<std::string> errors;

    for (const auto& res : resources) {
        exv::observability::LogFacade::info("[helper] Cleaning managed resource: type=" + res.type
                              + " detail=" + res.detail);
    }
    if (!resources.empty() && !network_ops_) {
        (void)policy;
        errors.push_back("Platform cleanup operations are not available");
        CleanupResponse cleanup_resp;
        cleanup_resp.success = false;
        cleanup_resp.errors = errors;
        return cleanup_resp;
    }
    if (!resources.empty()) {
        CleanupResponse cleanup_resp =
            network_ops_->cleanup(session_id, policy, resources);
        if (!cleanup_resp.success) {
            return cleanup_resp;
        }
    }

    cleanup_.remove_session(session_id);
    leases_.remove_session(session_id);

    CleanupResponse cleanup_resp;
    cleanup_resp.success = errors.empty();
    cleanup_resp.errors = errors;
    return cleanup_resp;
}

HelperResponse HelperHandler::handle_cleanup(const HelperRequest& req) {
    CleanupRequest cleanup_req;
    try {
        auto j = nlohmann::json::parse(req.payload_json);
        cleanup_req = cleanup_request_from_json(j);
    } catch (const std::exception& e) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = std::string("Failed to parse Cleanup request: ") + e.what();
        return resp;
    }

    if (!leases_.has_session(cleanup_req.session_id)) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_session";
        resp.error_message = "Session not found: " + cleanup_req.session_id.value;
        return resp;
    }

    exv::observability::LogFacade::info("[helper] Cleaning up session: " + cleanup_req.session_id.value);

    CleanupResponse cleanup_resp =
        cleanup_session(cleanup_req.session_id, cleanup_req.policy);

    nlohmann::json payload;
    to_json(payload, cleanup_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = cleanup_resp.success;
    if (!cleanup_resp.success) {
        resp.error_code = "cleanup_partial";
        resp.error_message = "Cleanup could not remove all managed resources";
    }
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperHandler::handle_get_snapshot(const HelperRequest& /*req*/) {
    auto now = std::chrono::steady_clock::now();

    GetSnapshotResponse snap_resp;

    // Iterate all active sessions and build snapshots
    auto records = cleanup_.all_records();
    for (const auto& record : records) {
        auto lease = leases_.get_session(record.session_id);
        if (!lease.has_value()) continue;

        SessionSnapshot snap;
        snap.session_id = record.session_id;
        snap.core_phase = lease->core_phase;
        snap.last_heartbeat = lease->last_heartbeat;

        auto resources = cleanup_.get_resources(record.session_id);
        for (const auto& res : resources) {
            snap.managed_resources.push_back(res.type + ":" + res.detail);
        }

        snap_resp.sessions.push_back(std::move(snap));
    }

    nlohmann::json payload;
    to_json(payload, snap_resp);

    HelperResponse resp;
    resp.op = HelperOp::GetSnapshot;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperHandler::handle_shutdown(const HelperRequest& req) {
    ShutdownRequest shutdown_req;
    try {
        auto j = nlohmann::json::parse(req.payload_json);
        shutdown_req = shutdown_request_from_json(j);
    } catch (const std::exception& e) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = std::string("Failed to parse Shutdown request: ") + e.what();
        return resp;
    }

    if (!leases_.has_session(shutdown_req.session_id)) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_session";
        resp.error_message = "Session not found: " + shutdown_req.session_id.value;
        return resp;
    }

    exv::observability::LogFacade::info("[helper] Shutting down session: " + shutdown_req.session_id.value);

    CleanupResponse cleanup_resp =
        cleanup_session(shutdown_req.session_id, shutdown_req.policy);

    ShutdownResponse shutdown_resp;
    shutdown_resp.cleanup_success = cleanup_resp.success;
    shutdown_resp.errors = cleanup_resp.errors;
    shutdown_resp.exiting =
        cleanup_resp.success && startup_context_.launch_mode == "oneshot";
    shutdown_requested_ = shutdown_resp.exiting;

    nlohmann::json payload;
    to_json(payload, shutdown_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = cleanup_resp.success;
    if (!cleanup_resp.success) {
        resp.error_code = "cleanup_partial";
        resp.error_message = "Shutdown cleanup could not remove all managed resources";
    }
    resp.payload_json = payload.dump();
    return resp;
}

} // namespace exv::helper
