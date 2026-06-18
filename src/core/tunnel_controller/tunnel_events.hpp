#pragma once

#include <string>

namespace exv::core {

enum class TunnelEventType {
    UserConnect,
    UserDisconnect,
    SetAutoReconnect,
    HelperReady,
    AuthSucceeded,
    AuthFailed,
    CstpConnected,
    NetworkConfigApplied,
    PacketLoopStarted,
    TransportClosed,
    PacketDeviceFailed,
    HelperLost,
    LeaseExpired,
    ReconnectTimerFired,
    CleanupSucceeded,
    CleanupFailed
};

struct TunnelEvent {
    TunnelEventType type;
    std::string code;
    std::string message;
    bool recoverable = false;
};

} // namespace exv::core
