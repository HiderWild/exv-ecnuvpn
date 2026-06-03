#pragma once
#include <string>
#include <vector>
#include <optional>
#include <cstdint>
#include <chrono>
#include <nlohmann/json.hpp>
#include "helper_protocol.hpp"

namespace exv::helper {

using json = nlohmann::json;

struct SessionId {
    std::string value;
};

inline bool operator==(const SessionId& a, const SessionId& b) {
    return a.value == b.value;
}

inline bool operator<(const SessionId& a, const SessionId& b) {
    return a.value < b.value;
}

struct ProfileId {
    std::string value;
};

// --- V2 Messages ---

struct HelloRequest {
    uint32_t client_version = PROTOCOL_VERSION;
};

struct HelloResponse {
    uint32_t server_version = PROTOCOL_VERSION;
    std::vector<std::string> capabilities;
    HelperMode mode = HelperMode::Transient;
};

struct StartSessionRequest {
    ProfileId profile_id;
    HelperMode mode = HelperMode::Transient;
};

struct StartSessionResponse {
    SessionId session_id;
};

struct PrepareTunnelDeviceRequest {
    SessionId session_id;
    std::string adapter_name;  // Platform-specific
    // No credentials - helper only creates device
};

struct PrepareTunnelDeviceResponse {
    std::string device_path;   // e.g. \\.\Wintun\... or /dev/tunN
    int mtu = 1400;
};

struct RouteEntry {
    std::string destination;   // e.g. "0.0.0.0/0"
    std::string gateway;
    int metric = 0;
};

struct DnsConfig {
    std::vector<std::string> servers;
    std::string search_domain;
};

struct TunnelConfig {
    SessionId session_id;
    std::string interface_address;  // e.g. "10.0.0.2/24"
    std::vector<RouteEntry> routes;
    DnsConfig dns;
    bool enable_kill_switch = false;
};

struct ApplyTunnelConfigRequest {
    TunnelConfig config;
};

struct ApplyTunnelConfigResponse {
    bool success = false;
    std::string error_message;
};

struct HeartbeatRequest {
    SessionId session_id;
    std::string core_phase;  // Connected|Reconnecting|etc.
};

struct HeartbeatResponse {
    bool ok = true;
    std::optional<std::string> warning;
};

struct CleanupPolicy {
    bool remove_routes = true;
    bool remove_dns = true;
    bool remove_adapter = false;  // Keep adapter for reconnect
    bool remove_firewall_rules = true;
};

struct CleanupRequest {
    SessionId session_id;
    CleanupPolicy policy;
};

struct CleanupResponse {
    bool success = false;
    std::vector<std::string> errors;  // Per-resource errors
};

struct GetSnapshotRequest {};

struct SessionSnapshot {
    SessionId session_id;
    std::string core_phase;
    std::chrono::steady_clock::time_point last_heartbeat;
    std::vector<std::string> managed_resources;
};

struct GetSnapshotResponse {
    std::vector<SessionSnapshot> sessions;
};

struct EndSessionRequest {
    SessionId session_id;
};

struct EndSessionResponse {
    bool success = false;
};

// --- Unified Request/Response ---

struct HelperRequest {
    HelperOp op;
    // Union of all request types (use variant in implementation)
    std::string payload_json;  // Serialized for now
};

struct HelperResponse {
    HelperOp op;
    bool success = false;
    std::string error_code;
    std::string error_message;
    std::string payload_json;  // Serialized for now
};

// --- Serialization (ADL-compatible to_json/from_json) ---

// SessionId / ProfileId
void to_json(json& j, const SessionId& id);
void from_json(const json& j, SessionId& id);
void to_json(json& j, const ProfileId& id);
void from_json(const json& j, ProfileId& id);

// Hello
void to_json(json& j, const HelloRequest& req);
HelloRequest hello_request_from_json(const json& j);
void to_json(json& j, const HelloResponse& resp);
HelloResponse hello_response_from_json(const json& j);

// StartSession
void to_json(json& j, const StartSessionRequest& req);
StartSessionRequest start_session_request_from_json(const json& j);
void to_json(json& j, const StartSessionResponse& resp);
StartSessionResponse start_session_response_from_json(const json& j);

// PrepareTunnelDevice
void to_json(json& j, const PrepareTunnelDeviceRequest& req);
PrepareTunnelDeviceRequest prepare_tunnel_device_request_from_json(const json& j);
void to_json(json& j, const PrepareTunnelDeviceResponse& resp);
PrepareTunnelDeviceResponse prepare_tunnel_device_response_from_json(const json& j);

// RouteEntry / DnsConfig / TunnelConfig
void to_json(json& j, const RouteEntry& r);
RouteEntry route_entry_from_json(const json& j);
void to_json(json& j, const DnsConfig& d);
DnsConfig dns_config_from_json(const json& j);
void to_json(json& j, const TunnelConfig& cfg);
TunnelConfig tunnel_config_from_json(const json& j);

// ApplyTunnelConfig
void to_json(json& j, const ApplyTunnelConfigRequest& req);
ApplyTunnelConfigRequest apply_tunnel_config_request_from_json(const json& j);
void to_json(json& j, const ApplyTunnelConfigResponse& resp);
ApplyTunnelConfigResponse apply_tunnel_config_response_from_json(const json& j);

// Heartbeat
void to_json(json& j, const HeartbeatRequest& req);
HeartbeatRequest heartbeat_request_from_json(const json& j);
void to_json(json& j, const HeartbeatResponse& resp);
HeartbeatResponse heartbeat_response_from_json(const json& j);

// Cleanup
void to_json(json& j, const CleanupPolicy& p);
CleanupPolicy cleanup_policy_from_json(const json& j);
void to_json(json& j, const CleanupRequest& req);
CleanupRequest cleanup_request_from_json(const json& j);
void to_json(json& j, const CleanupResponse& resp);
CleanupResponse cleanup_response_from_json(const json& j);

// Snapshot
void to_json(json& j, const GetSnapshotRequest& req);
GetSnapshotRequest get_snapshot_request_from_json(const json& j);
void to_json(json& j, const SessionSnapshot& snap);
SessionSnapshot session_snapshot_from_json(const json& j);
void to_json(json& j, const GetSnapshotResponse& resp);
GetSnapshotResponse get_snapshot_response_from_json(const json& j);

// EndSession
void to_json(json& j, const EndSessionRequest& req);
EndSessionRequest end_session_request_from_json(const json& j);
void to_json(json& j, const EndSessionResponse& resp);
EndSessionResponse end_session_response_from_json(const json& j);

// Unified envelope
void to_json(json& j, const HelperRequest& req);
HelperRequest helper_request_from_json(const json& j);
void to_json(json& j, const HelperResponse& resp);
HelperResponse helper_response_from_json(const json& j);

} // namespace exv::helper
