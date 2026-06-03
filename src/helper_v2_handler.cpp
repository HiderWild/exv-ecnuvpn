#include "helper_v2_handler.hpp"
#include "logger.hpp"

#include <nlohmann/json.hpp>
#include <sstream>

namespace exv::helper {

HelperV2Handler::HelperV2Handler() {
    register_handlers();
}

void HelperV2Handler::register_handlers() {
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
    dispatcher_.register_handler(HelperOp::EndSession,
        [this](const HelperRequest& req) { return handle_end_session(req); });
}

HelperResponse HelperV2Handler::handle(const HelperRequest& request) {
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

void HelperV2Handler::tick() {
    auto now = std::chrono::steady_clock::now();
    auto expired = leases_.find_expired_sessions(now);
    for (const auto& id : expired) {
        auto lease = leases_.get_session(id);
        if (lease.has_value()) {
            ecnuvpn::logger::info("[v2-handler] Cleaning up expired session: " + id.value);
            cleanup_.remove_session(id);
            leases_.remove_session(id);
        }
    }
}

SessionLeaseManager& HelperV2Handler::lease_manager() {
    return leases_;
}

CleanupRegistry& HelperV2Handler::cleanup_registry() {
    return cleanup_;
}

// --- Handler implementations ---

HelperResponse HelperV2Handler::handle_hello(const HelperRequest& req) {
    nlohmann::json payload;
    payload["server_version"] = PROTOCOL_VERSION;
    payload["capabilities"] = nlohmann::json::array({
        "tunnel_device", "tunnel_config", "session_management",
        "heartbeat", "cleanup", "snapshot"
    });
    payload["mode"] = "transient";

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperV2Handler::handle_start_session(const HelperRequest& req) {
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

    CleanupPolicy default_policy;
    SessionId session_id = leases_.create_session(
        start_req.profile_id, start_req.mode, default_policy);

    // Register a cleanup record for the new session
    CleanupRecord record;
    record.session_id = session_id;
    record.created_at = std::chrono::system_clock::now();
    cleanup_.register_session(record);

    ecnuvpn::logger::info("[v2-handler] Session started: " + session_id.value);

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

HelperResponse HelperV2Handler::handle_prepare_tunnel_device(const HelperRequest& req) {
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

    ecnuvpn::logger::info("[v2-handler] PrepareTunnelDevice (stub) for session: "
                          + device_req.session_id.value
                          + " adapter=" + device_req.adapter_name);

    // Stub: return a placeholder device path
    PrepareTunnelDeviceResponse device_resp;
    device_resp.device_path = "\\\\.\\Wintun\\ecnuvpn";
    device_resp.mtu = 1400;

    nlohmann::json payload;
    to_json(payload, device_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperV2Handler::handle_apply_tunnel_config(const HelperRequest& req) {
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

    ecnuvpn::logger::info("[v2-handler] ApplyTunnelConfig (stub) for session: "
                          + config_req.config.session_id.value
                          + " address=" + config_req.config.interface_address
                          + " routes=" + std::to_string(config_req.config.routes.size()));

    // Track routes in cleanup registry
    for (const auto& route : config_req.config.routes) {
        cleanup_.add_resource(config_req.config.session_id,
            {"route", route.destination + " via " + route.gateway});
    }
    for (const auto& dns_server : config_req.config.dns.servers) {
        cleanup_.add_resource(config_req.config.session_id, {"dns", dns_server});
    }

    ApplyTunnelConfigResponse config_resp;
    config_resp.success = true;

    nlohmann::json payload;
    to_json(payload, config_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperV2Handler::handle_heartbeat(const HelperRequest& req) {
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

HelperResponse HelperV2Handler::handle_cleanup(const HelperRequest& req) {
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

    ecnuvpn::logger::info("[v2-handler] Cleaning up session: " + cleanup_req.session_id.value);

    auto resources = cleanup_.get_resources(cleanup_req.session_id);
    std::vector<std::string> errors;

    // Stub: log what would be cleaned up
    for (const auto& res : resources) {
        ecnuvpn::logger::info("[v2-handler] Would clean up resource: type=" + res.type
                              + " detail=" + res.detail);
    }

    // Remove from tracking
    cleanup_.remove_session(cleanup_req.session_id);
    leases_.remove_session(cleanup_req.session_id);

    CleanupResponse cleanup_resp;
    cleanup_resp.success = true;
    cleanup_resp.errors = errors;

    nlohmann::json payload;
    to_json(payload, cleanup_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

HelperResponse HelperV2Handler::handle_get_snapshot(const HelperRequest& /*req*/) {
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

HelperResponse HelperV2Handler::handle_end_session(const HelperRequest& req) {
    EndSessionRequest end_req;
    try {
        auto j = nlohmann::json::parse(req.payload_json);
        end_req = end_session_request_from_json(j);
    } catch (const std::exception& e) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = std::string("Failed to parse EndSession request: ") + e.what();
        return resp;
    }

    if (!leases_.has_session(end_req.session_id)) {
        HelperResponse resp;
        resp.op = req.op;
        resp.success = false;
        resp.error_code = "invalid_session";
        resp.error_message = "Session not found: " + end_req.session_id.value;
        return resp;
    }

    ecnuvpn::logger::info("[v2-handler] Ending session: " + end_req.session_id.value);

    cleanup_.remove_session(end_req.session_id);
    leases_.remove_session(end_req.session_id);

    EndSessionResponse end_resp;
    end_resp.success = true;

    nlohmann::json payload;
    to_json(payload, end_resp);

    HelperResponse resp;
    resp.op = req.op;
    resp.success = true;
    resp.payload_json = payload.dump();
    return resp;
}

} // namespace exv::helper
