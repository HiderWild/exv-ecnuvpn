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
    std::string helper_status;    // connected|unavailable|version_mismatch
    bool network_ready = false;
    std::string server;
    std::string interface_name;
    std::optional<ErrorInfo> last_error;
    std::optional<ReconnectInfo> reconnect;
};

} // namespace exv::core
