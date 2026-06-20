#pragma once
#include <string>
#include <optional>
#include "tunnel_intent.hpp"

namespace exv::core {

enum class TunnelPhase {
    Idle,
    PreparingHelper,
    Authenticating,
    ConnectingCstp,
    ApplyingNetworkConfig,
    OpeningPacketDevice,
    Connected,
    Reconnecting,
    Disconnecting,
    CleaningUp,
    Failed
};

inline constexpr const char* tunnel_phase_wire_name(TunnelPhase phase) noexcept {
    switch (phase) {
    case TunnelPhase::Idle: return "idle";
    case TunnelPhase::PreparingHelper: return "preparing_helper";
    case TunnelPhase::Authenticating: return "authenticating";
    case TunnelPhase::ConnectingCstp: return "connecting_cstp";
    case TunnelPhase::ApplyingNetworkConfig: return "applying_network_config";
    case TunnelPhase::OpeningPacketDevice: return "opening_packet_device";
    case TunnelPhase::Connected: return "connected";
    case TunnelPhase::Reconnecting: return "reconnecting";
    case TunnelPhase::Disconnecting: return "disconnecting";
    case TunnelPhase::CleaningUp: return "cleaning_up";
    case TunnelPhase::Failed: return "failed";
    }
    return "unknown";
}

struct ErrorInfo {
    std::string domain;    // transport|auth|helper|os.route|os.dns|packet
    std::string code;      // transport_closed, auth_failed, etc.
    std::string message;
    std::optional<int> native_code;
    std::string native_api;
    bool recoverable = false;
    std::string recommended_action;
};

struct ReconnectInfo {
    int attempt = 0;
    int next_retry_ms = 0;
};

struct TunnelStatusSnapshot {
    TunnelPhase phase = TunnelPhase::Idle;
    bool desired_connected = false;
    bool auto_reconnect = true;
    std::string helper_mode;      // transient|resident
    std::string helper_status;    // connected|unavailable|permission_denied
    std::string helper_endpoint;
    bool core_lease_active = false;
    bool session_active = false;
    bool network_ready = false;
    std::string server;
    std::string interface_name;
    std::string internal_ip;
    std::optional<ErrorInfo> last_error;
    std::optional<ReconnectInfo> reconnect;
};

} // namespace exv::core
