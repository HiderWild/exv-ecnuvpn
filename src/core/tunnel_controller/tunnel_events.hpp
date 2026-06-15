#pragma once

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
    // Payload will be added in Phase 2
};

} // namespace exv::core
