#pragma once

#include "vpn_engine/event_sink.hpp"
#include "tunnel_events.hpp"

#include <functional>

namespace exv::core {

// Bridges NativeVpnEngineSession events (VpnEngineEvent) to TunnelEvent,
// forwarding them to TunnelController via a callback.
//
// This class implements vpn_engine::EventSink so it can be injected into
// NativeVpnEngineDependencies::event_sink.  When the engine emits an event,
// map_event() translates the engine event type string into a TunnelEventType
// and invokes the registered callback.
//
// Thread-safety: emit() may be called from both the caller thread (auth/CSTP
// events) and the packet-loop thread.  The callback must be safe to call from
// either context.
class EngineEventBridge final : public exv::vpn_engine::EventSink {
public:
    using EventCallback = std::function<void(TunnelEvent)>;

    explicit EngineEventBridge(EventCallback callback);
    ~EngineEventBridge() override;

    // exv::vpn_engine::EventSink interface
    void emit(const exv::vpn_engine::VpnEngineEvent& event) override;

    // Visible for testing: maps an engine event type string to a TunnelEventType.
    // Returns true and sets *out on success; returns false if the event type
    // is not recognised (silently ignored by emit()).
    static bool map_event(const std::string& engine_type, TunnelEventType* out);

private:
    EventCallback callback_;
};

} // namespace exv::core
