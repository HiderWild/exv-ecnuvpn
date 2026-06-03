#include "helper_messages.hpp"

namespace exv::helper {

// ---- HelperMode / HelperOp conversions ----

namespace {

uint32_t mode_to_uint(HelperMode m) {
    return static_cast<uint32_t>(m);
}

HelperMode uint_to_mode(uint32_t v) {
    return static_cast<HelperMode>(v);
}

uint32_t op_to_uint(HelperOp o) {
    return static_cast<uint32_t>(o);
}

HelperOp uint_to_op(uint32_t v) {
    return static_cast<HelperOp>(v);
}

} // anonymous namespace

// ---- SessionId / ProfileId ----

void to_json(json& j, const SessionId& id) {
    j = id.value;
}

void from_json(const json& j, SessionId& id) {
    id.value = j.get<std::string>();
}

void to_json(json& j, const ProfileId& id) {
    j = id.value;
}

void from_json(const json& j, ProfileId& id) {
    id.value = j.get<std::string>();
}

// ---- HelloRequest / HelloResponse ----

void to_json(json& j, const HelloRequest& req) {
    j = json{{"client_version", req.client_version}};
}

HelloRequest hello_request_from_json(const json& j) {
    HelloRequest req;
    req.client_version = j.value("client_version", PROTOCOL_VERSION);
    return req;
}

void to_json(json& j, const HelloResponse& resp) {
    j = json{{"server_version", resp.server_version},
             {"capabilities", resp.capabilities},
             {"mode", mode_to_uint(resp.mode)}};
}

HelloResponse hello_response_from_json(const json& j) {
    HelloResponse resp;
    resp.server_version = j.value("server_version", PROTOCOL_VERSION);
    if (j.contains("capabilities"))
        resp.capabilities = j["capabilities"].get<std::vector<std::string>>();
    if (j.contains("mode"))
        resp.mode = uint_to_mode(j["mode"].get<uint32_t>());
    return resp;
}

// ---- StartSessionRequest / StartSessionResponse ----

void to_json(json& j, const StartSessionRequest& req) {
    j = json{{"profile_id", req.profile_id}, {"mode", mode_to_uint(req.mode)}};
}

StartSessionRequest start_session_request_from_json(const json& j) {
    StartSessionRequest req;
    if (j.contains("profile_id")) from_json(j["profile_id"], req.profile_id);
    if (j.contains("mode")) req.mode = uint_to_mode(j["mode"].get<uint32_t>());
    return req;
}

void to_json(json& j, const StartSessionResponse& resp) {
    j = json{{"session_id", resp.session_id}};
}

StartSessionResponse start_session_response_from_json(const json& j) {
    StartSessionResponse resp;
    if (j.contains("session_id")) from_json(j["session_id"], resp.session_id);
    return resp;
}

// ---- PrepareTunnelDeviceRequest / PrepareTunnelDeviceResponse ----

void to_json(json& j, const PrepareTunnelDeviceRequest& req) {
    j = json{{"session_id", req.session_id}, {"adapter_name", req.adapter_name}};
}

PrepareTunnelDeviceRequest prepare_tunnel_device_request_from_json(const json& j) {
    PrepareTunnelDeviceRequest req;
    if (j.contains("session_id")) from_json(j["session_id"], req.session_id);
    req.adapter_name = j.value("adapter_name", "");
    return req;
}

void to_json(json& j, const PrepareTunnelDeviceResponse& resp) {
    j = json{{"device_path", resp.device_path}, {"mtu", resp.mtu}};
}

PrepareTunnelDeviceResponse prepare_tunnel_device_response_from_json(const json& j) {
    PrepareTunnelDeviceResponse resp;
    resp.device_path = j.value("device_path", "");
    resp.mtu = j.value("mtu", 1400);
    return resp;
}

// ---- RouteEntry ----

void to_json(json& j, const RouteEntry& r) {
    j = json{{"destination", r.destination},
             {"gateway", r.gateway},
             {"metric", r.metric}};
}

RouteEntry route_entry_from_json(const json& j) {
    RouteEntry r;
    r.destination = j.value("destination", "");
    r.gateway = j.value("gateway", "");
    r.metric = j.value("metric", 0);
    return r;
}

// ---- DnsConfig ----

void to_json(json& j, const DnsConfig& d) {
    j = json{{"servers", d.servers}, {"search_domain", d.search_domain}};
}

DnsConfig dns_config_from_json(const json& j) {
    DnsConfig d;
    if (j.contains("servers")) d.servers = j["servers"].get<std::vector<std::string>>();
    d.search_domain = j.value("search_domain", "");
    return d;
}

// ---- TunnelConfig ----

void to_json(json& j, const TunnelConfig& cfg) {
    json routes = json::array();
    for (const auto& r : cfg.routes) routes.push_back(r);
    j = json{{"session_id", cfg.session_id},
             {"interface_address", cfg.interface_address},
             {"routes", routes},
             {"dns", cfg.dns},
             {"enable_kill_switch", cfg.enable_kill_switch}};
}

TunnelConfig tunnel_config_from_json(const json& j) {
    TunnelConfig cfg;
    if (j.contains("session_id")) from_json(j["session_id"], cfg.session_id);
    cfg.interface_address = j.value("interface_address", "");
    if (j.contains("routes")) {
        for (const auto& rj : j["routes"])
            cfg.routes.push_back(route_entry_from_json(rj));
    }
    if (j.contains("dns")) cfg.dns = dns_config_from_json(j["dns"]);
    cfg.enable_kill_switch = j.value("enable_kill_switch", false);
    return cfg;
}

// ---- ApplyTunnelConfigRequest / ApplyTunnelConfigResponse ----

void to_json(json& j, const ApplyTunnelConfigRequest& req) {
    j = json{{"config", req.config}};
}

ApplyTunnelConfigRequest apply_tunnel_config_request_from_json(const json& j) {
    ApplyTunnelConfigRequest req;
    if (j.contains("config")) req.config = tunnel_config_from_json(j["config"]);
    return req;
}

void to_json(json& j, const ApplyTunnelConfigResponse& resp) {
    j = json{{"success", resp.success}, {"error_message", resp.error_message}};
}

ApplyTunnelConfigResponse apply_tunnel_config_response_from_json(const json& j) {
    ApplyTunnelConfigResponse resp;
    resp.success = j.value("success", false);
    resp.error_message = j.value("error_message", "");
    return resp;
}

// ---- HeartbeatRequest / HeartbeatResponse ----

void to_json(json& j, const HeartbeatRequest& req) {
    j = json{{"session_id", req.session_id}, {"core_phase", req.core_phase}};
}

HeartbeatRequest heartbeat_request_from_json(const json& j) {
    HeartbeatRequest req;
    if (j.contains("session_id")) from_json(j["session_id"], req.session_id);
    req.core_phase = j.value("core_phase", "");
    return req;
}

void to_json(json& j, const HeartbeatResponse& resp) {
    j = json{{"ok", resp.ok}};
    if (resp.warning.has_value()) j["warning"] = resp.warning.value();
}

HeartbeatResponse heartbeat_response_from_json(const json& j) {
    HeartbeatResponse resp;
    resp.ok = j.value("ok", true);
    if (j.contains("warning")) resp.warning = j["warning"].get<std::string>();
    return resp;
}

// ---- CleanupPolicy ----

void to_json(json& j, const CleanupPolicy& p) {
    j = json{{"remove_routes", p.remove_routes},
             {"remove_dns", p.remove_dns},
             {"remove_adapter", p.remove_adapter},
             {"remove_firewall_rules", p.remove_firewall_rules}};
}

CleanupPolicy cleanup_policy_from_json(const json& j) {
    CleanupPolicy p;
    p.remove_routes = j.value("remove_routes", true);
    p.remove_dns = j.value("remove_dns", true);
    p.remove_adapter = j.value("remove_adapter", false);
    p.remove_firewall_rules = j.value("remove_firewall_rules", true);
    return p;
}

// ---- CleanupRequest / CleanupResponse ----

void to_json(json& j, const CleanupRequest& req) {
    j = json{{"session_id", req.session_id}, {"policy", req.policy}};
}

CleanupRequest cleanup_request_from_json(const json& j) {
    CleanupRequest req;
    if (j.contains("session_id")) from_json(j["session_id"], req.session_id);
    if (j.contains("policy")) req.policy = cleanup_policy_from_json(j["policy"]);
    return req;
}

void to_json(json& j, const CleanupResponse& resp) {
    j = json{{"success", resp.success}, {"errors", resp.errors}};
}

CleanupResponse cleanup_response_from_json(const json& j) {
    CleanupResponse resp;
    resp.success = j.value("success", false);
    if (j.contains("errors")) resp.errors = j["errors"].get<std::vector<std::string>>();
    return resp;
}

// ---- GetSnapshotRequest / SessionSnapshot / GetSnapshotResponse ----

void to_json(json& /*j*/, const GetSnapshotRequest& /*req*/) {
    // No fields
}

GetSnapshotRequest get_snapshot_request_from_json(const json& /*j*/) {
    return {};
}

void to_json(json& j, const SessionSnapshot& snap) {
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
        snap.last_heartbeat.time_since_epoch())
        .count();
    j = json{{"session_id", snap.session_id},
             {"core_phase", snap.core_phase},
             {"last_heartbeat_ms", ms},
             {"managed_resources", snap.managed_resources}};
}

SessionSnapshot session_snapshot_from_json(const json& j) {
    SessionSnapshot snap;
    if (j.contains("session_id")) from_json(j["session_id"], snap.session_id);
    snap.core_phase = j.value("core_phase", "");
    if (j.contains("last_heartbeat_ms")) {
        auto ms = j["last_heartbeat_ms"].get<int64_t>();
        snap.last_heartbeat = std::chrono::steady_clock::time_point(
            std::chrono::milliseconds(ms));
    }
    if (j.contains("managed_resources"))
        snap.managed_resources = j["managed_resources"].get<std::vector<std::string>>();
    return snap;
}

void to_json(json& j, const GetSnapshotResponse& resp) {
    json arr = json::array();
    for (const auto& s : resp.sessions) arr.push_back(s);
    j = json{{"sessions", arr}};
}

GetSnapshotResponse get_snapshot_response_from_json(const json& j) {
    GetSnapshotResponse resp;
    if (j.contains("sessions")) {
        for (const auto& sj : j["sessions"])
            resp.sessions.push_back(session_snapshot_from_json(sj));
    }
    return resp;
}

// ---- EndSessionRequest / EndSessionResponse ----

void to_json(json& j, const EndSessionRequest& req) {
    j = json{{"session_id", req.session_id}};
}

EndSessionRequest end_session_request_from_json(const json& j) {
    EndSessionRequest req;
    if (j.contains("session_id")) from_json(j["session_id"], req.session_id);
    return req;
}

void to_json(json& j, const EndSessionResponse& resp) {
    j = json{{"success", resp.success}};
}

EndSessionResponse end_session_response_from_json(const json& j) {
    EndSessionResponse resp;
    resp.success = j.value("success", false);
    return resp;
}

// ---- HelperRequest / HelperResponse (unified envelope) ----

void to_json(json& j, const HelperRequest& req) {
    j = json{{"op", op_to_uint(req.op)}, {"payload_json", req.payload_json}};
}

HelperRequest helper_request_from_json(const json& j) {
    HelperRequest req;
    if (j.contains("op")) req.op = uint_to_op(j["op"].get<uint32_t>());
    req.payload_json = j.value("payload_json", "");
    return req;
}

void to_json(json& j, const HelperResponse& resp) {
    j = json{{"op", op_to_uint(resp.op)},
             {"success", resp.success},
             {"error_code", resp.error_code},
             {"error_message", resp.error_message},
             {"payload_json", resp.payload_json}};
}

HelperResponse helper_response_from_json(const json& j) {
    HelperResponse resp;
    if (j.contains("op")) resp.op = uint_to_op(j["op"].get<uint32_t>());
    resp.success = j.value("success", false);
    resp.error_code = j.value("error_code", "");
    resp.error_message = j.value("error_message", "");
    resp.payload_json = j.value("payload_json", "");
    return resp;
}

} // namespace exv::helper
