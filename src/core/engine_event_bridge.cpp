#include "engine_event_bridge.hpp"

namespace exv::core {

EngineEventBridge::EngineEventBridge(EventCallback callback)
    : callback_(std::move(callback)) {}

EngineEventBridge::~EngineEventBridge() = default;

void EngineEventBridge::emit(const ecnuvpn::vpn_engine::VpnEngineEvent& event) {
    TunnelEventType type{};
    if (!map_event(event.type, &type)) {
        return;  // Unrecognised engine event -- silently skip.
    }
    if (callback_) {
        callback_(TunnelEvent{type});
    }
}

bool EngineEventBridge::map_event(const std::string& engine_type,
                                  TunnelEventType* out) {
    if (engine_type == "auth.succeeded") {
        *out = TunnelEventType::AuthSucceeded;
        return true;
    }
    if (engine_type == "auth.failed") {
        *out = TunnelEventType::AuthFailed;
        return true;
    }
    if (engine_type == "cstp.connected") {
        *out = TunnelEventType::CstpConnected;
        return true;
    }
    if (engine_type == "packet.loop.started") {
        *out = TunnelEventType::PacketLoopStarted;
        return true;
    }
    if (engine_type == "transport.closed") {
        *out = TunnelEventType::TransportClosed;
        return true;
    }
    if (engine_type == "packet_device.failed") {
        *out = TunnelEventType::PacketDeviceFailed;
        return true;
    }
    return false;
}

} // namespace exv::core
