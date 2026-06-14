// Generated from contracts/system.contract.json. Do not edit manually.
#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <string_view>

namespace exv::contracts::generated {

inline constexpr std::string_view CONTRACT_VERSION = "2026-06-14.config-helper-contract.v1";

inline constexpr std::array<std::string_view, 3> DESKTOP_RPC_REQUEST_FIELDS = {{"id", "action", "payload"}};
inline constexpr std::array<std::string_view, 5> DESKTOP_RPC_RESPONSE_FIELDS = {{"ok", "data", "code", "message", "event"}};
inline constexpr std::array<std::string_view, 3> CORE_RPC_REQUEST_FIELDS = {{"action", "payload_json", "request_id"}};
inline constexpr std::array<std::string_view, 5> CORE_RPC_RESPONSE_FIELDS = {{"success", "payload_json", "error_code", "error_message", "request_id"}};

inline constexpr std::array<std::string_view, 18> DESKTOP_RPC_ACTIONS = {{"status.get", "vpn.connect", "vpn.disconnect", "config.getAuth", "config.saveAuth", "config.getSettings", "config.saveSettings", "config.getKey", "routes.list", "routes.add", "routes.remove", "routes.reset", "service.status", "helper.status", "runtime.status", "drivers.status", "drivers.install", "logs.list"}};
inline constexpr std::array<std::string_view, 6> DESKTOP_RPC_EVENT_TYPES = {{"log", "status", "heartbeat", "service-progress", "close-request", "core-crashed"}};
inline constexpr std::array<std::string_view, 18> DESKTOP_RPC_ERROR_CODES = {{"helper_unavailable", "service_not_installed", "service_installed_not_running", "service_start_failed", "oneshot_not_supported", "oneshot_elevation_denied", "helper_rpc_failed", "auth_failed", "tls_verify_failed", "wintun_missing", "utun_permission_denied", "unsupported_dtls", "permission_denied", "network_unreachable", "user_cancelled", "invalid_request", "connection_failed", "vpn_start_failed"}};
inline constexpr std::array<std::string_view, 7> CONFIG_ACTIONS = {{"config.getAuth", "config.saveAuth", "config.getSettings", "config.saveSettings", "config.getKey", "config.profile.get", "config.profile.save"}};
inline constexpr std::array<std::string_view, 4> CONFIG_LEGACY_ALIASES = {{"config.get", "config.save", "config.get_profile", "config.save_profile"}};
inline constexpr std::array<std::string_view, 8> HELPER_OPS = {{"Hello", "StartSession", "PrepareTunnelDevice", "ApplyTunnelConfig", "Heartbeat", "Cleanup", "GetSnapshot", "Shutdown"}};
inline constexpr std::array<std::string_view, 14> HELPER_FORBIDDEN_CREDENTIAL_FIELDS = {{"password", "passwd", "cookie", "token", "secret", "credential", "auth_key", "auth_token", "session_cookie", "webvpn_cookie", "csrf_token", "bearer_token", "api_key", "apikey"}};

struct HelperOpContract {
    std::string_view name;
    std::uint32_t code;
    bool requires_session;
};

inline constexpr std::array<HelperOpContract, 8> HELPER_OP_CONTRACTS = {{
    {"Hello", 1, false},
    {"StartSession", 2, false},
    {"PrepareTunnelDevice", 3, true},
    {"ApplyTunnelConfig", 4, true},
    {"Heartbeat", 5, true},
    {"Cleanup", 6, true},
    {"GetSnapshot", 7, false},
    {"Shutdown", 8, true},
}};

struct ConfigAlias {
    std::string_view alias;
    std::string_view target;
};

inline constexpr std::array<ConfigAlias, 4> CONFIG_ALIASES = {{
    {"config.get", "config.getSettings"},
    {"config.save", "config.saveSettings"},
    {"config.get_profile", "config.profile.get"},
    {"config.save_profile", "config.profile.save"},
}};

template <std::size_t N>
constexpr bool contains(const std::array<std::string_view, N>& values, std::string_view value) {
    for (const auto item : values) {
        if (item == value) {
            return true;
        }
    }
    return false;
}

constexpr bool is_desktop_rpc_action(std::string_view action) {
    return contains(DESKTOP_RPC_ACTIONS, action);
}

constexpr bool is_config_action(std::string_view action) {
    return contains(CONFIG_ACTIONS, action);
}

constexpr bool is_config_alias(std::string_view alias) {
    return contains(CONFIG_LEGACY_ALIASES, alias);
}

constexpr bool is_helper_op(std::string_view op) {
    return contains(HELPER_OPS, op);
}

constexpr bool is_helper_forbidden_credential_field(std::string_view field) {
    return contains(HELPER_FORBIDDEN_CREDENTIAL_FIELDS, field);
}

} // namespace exv::contracts::generated
