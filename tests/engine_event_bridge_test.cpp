#include "core/tunnel_controller/engine_event_bridge.hpp"
#include "common/diagnostics/log_event_bus.hpp"

#include <algorithm>
#include <iostream>
#include <string>
#include <vector>

import exv.core.tunnel.events;

namespace events = exv::core::tunnel::events;

namespace {

bool expect(bool condition, const char* message) {
    if (condition)
        return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

} // namespace

int main() {
    bool ok = true;

    // ---------------------------------------------------------------
    // 1. map_event: known engine types map to the correct TunnelEventType
    // ---------------------------------------------------------------

    {
        events::TunnelEventType out{};
        ok = expect(exv::core::EngineEventBridge::map_event("auth.succeeded", &out) &&
                    out == events::TunnelEventType::AuthSucceeded,
                    "auth.succeeded -> AuthSucceeded") && ok;
    }
    {
        events::TunnelEventType out{};
        ok = expect(exv::core::EngineEventBridge::map_event("auth.failed", &out) &&
                    out == events::TunnelEventType::AuthFailed,
                    "auth.failed -> AuthFailed") && ok;
    }
    {
        events::TunnelEventType out{};
        ok = expect(exv::core::EngineEventBridge::map_event("cstp.connected", &out) &&
                    out == events::TunnelEventType::CstpConnected,
                    "cstp.connected -> CstpConnected") && ok;
    }
    {
        events::TunnelEventType out{};
        ok = expect(exv::core::EngineEventBridge::map_event("packet.loop.started", &out) &&
                    out == events::TunnelEventType::PacketLoopStarted,
                    "packet.loop.started -> PacketLoopStarted") && ok;
    }
    {
        events::TunnelEventType out{};
        ok = expect(exv::core::EngineEventBridge::map_event("transport.closed", &out) &&
                    out == events::TunnelEventType::TransportClosed,
                    "transport.closed -> TransportClosed") && ok;
    }
    {
        events::TunnelEventType out{};
        ok = expect(exv::core::EngineEventBridge::map_event("packet_device.failed", &out) &&
                    out == events::TunnelEventType::PacketDeviceFailed,
                    "packet_device.failed -> PacketDeviceFailed") && ok;
    }

    // ---------------------------------------------------------------
    // 2. map_event: unknown engine types return false
    // ---------------------------------------------------------------

    {
        events::TunnelEventType out{};
        ok = expect(!exv::core::EngineEventBridge::map_event("unknown.event", &out),
                    "unknown.event -> false") && ok;
    }
    {
        events::TunnelEventType out{};
        ok = expect(!exv::core::EngineEventBridge::map_event("auth.started", &out),
                    "auth.started -> false (not mapped)") && ok;
    }
    {
        events::TunnelEventType out{};
        ok = expect(!exv::core::EngineEventBridge::map_event("packet.inbound", &out),
                    "packet.inbound -> false (not mapped)") && ok;
    }

    // ---------------------------------------------------------------
    // 3. emit(): callback receives the correct TunnelEvent
    // ---------------------------------------------------------------

    {
        std::vector<events::TunnelEventType> received;
        exv::core::EngineEventBridge bridge(
            [&](events::TunnelEvent e) { received.push_back(e.type); });

        ecnuvpn::vpn_engine::VpnEngineEvent ev1;
        ev1.type = "auth.succeeded";
        bridge.emit(ev1);

        ecnuvpn::vpn_engine::VpnEngineEvent ev2;
        ev2.type = "cstp.connected";
        bridge.emit(ev2);

        ok = expect(received.size() == 2, "callback invoked twice") && ok;
        ok = expect(received[0] == events::TunnelEventType::AuthSucceeded,
                    "first event is AuthSucceeded") && ok;
        ok = expect(received[1] == events::TunnelEventType::CstpConnected,
                    "second event is CstpConnected") && ok;
    }

    // ---------------------------------------------------------------
    // 4. emit(): unknown events are silently skipped
    // ---------------------------------------------------------------

    {
        int call_count = 0;
        exv::core::EngineEventBridge bridge(
            [&](events::TunnelEvent) { ++call_count; });

        ecnuvpn::vpn_engine::VpnEngineEvent ev;
        ev.type = "native.starting";
        bridge.emit(ev);

        ok = expect(call_count == 0, "unknown event should not trigger callback") && ok;
    }

    // ---------------------------------------------------------------
    // 5. emit(): all six mapped event types round-trip correctly
    // ---------------------------------------------------------------

    {
        std::vector<events::TunnelEventType> received;
        exv::core::EngineEventBridge bridge(
            [&](events::TunnelEvent e) { received.push_back(e.type); });

        const char* types[] = {
            "auth.succeeded",
            "auth.failed",
            "cstp.connected",
            "packet.loop.started",
            "transport.closed",
            "packet_device.failed"
        };
        for (const char* t : types) {
            ecnuvpn::vpn_engine::VpnEngineEvent ev;
            ev.type = t;
            bridge.emit(ev);
        }

        ok = expect(received.size() == 6, "all 6 mapped types received") && ok;
        ok = expect(received[0] == events::TunnelEventType::AuthSucceeded,
                    "index 0: AuthSucceeded") && ok;
        ok = expect(received[1] == events::TunnelEventType::AuthFailed,
                    "index 1: AuthFailed") && ok;
        ok = expect(received[2] == events::TunnelEventType::CstpConnected,
                    "index 2: CstpConnected") && ok;
        ok = expect(received[3] == events::TunnelEventType::PacketLoopStarted,
                    "index 3: PacketLoopStarted") && ok;
        ok = expect(received[4] == events::TunnelEventType::TransportClosed,
                    "index 4: TransportClosed") && ok;
        ok = expect(received[5] == events::TunnelEventType::PacketDeviceFailed,
                    "index 5: PacketDeviceFailed") && ok;
    }

    // ---------------------------------------------------------------
    // 6. emit(): null callback does not crash
    // ---------------------------------------------------------------

    {
        exv::core::EngineEventBridge bridge(nullptr);
        ecnuvpn::vpn_engine::VpnEngineEvent ev;
        ev.type = "auth.succeeded";
        bridge.emit(ev);  // Should not crash
        ok = expect(true, "null callback does not crash") && ok;
    }

    // ---------------------------------------------------------------
    // 7. emit(): lifecycle events publish user-facing engine logs,
    //    while packet/DPD noise is filtered out.
    // ---------------------------------------------------------------

    {
        std::vector<ecnuvpn::TypedLogEvent> logs;
        auto subscription = ecnuvpn::LogEventBus::instance().subscribe(
            [&](const ecnuvpn::TypedLogEvent& event) { logs.push_back(event); });

        exv::core::EngineEventBridge bridge(nullptr);
        const char* log_worthy_types[] = {
            "native.starting",
            "auth.started",
            "auth.succeeded",
            "auth.failed",
            "cstp.connected",
            "cstp.failed",
            "packet.loop.started",
            "packet_device.failed",
            "transport.closed",
            "packet.loop.stopped",
            "reconnect.scheduled",
            "reconnect.started"
        };
        for (const char* t : log_worthy_types) {
            ecnuvpn::vpn_engine::VpnEngineEvent ev;
            ev.type = t;
            ev.message = std::string("message for ") + t;
            ev.fields.emplace("phase", "test_phase");
            bridge.emit(ev);
        }

        const char* noisy_types[] = {
            "packet.inbound",
            "packet.outbound",
            "dpd.sent",
            "dpd.received",
            "dpd.timeout"
        };
        for (const char* t : noisy_types) {
            ecnuvpn::vpn_engine::VpnEngineEvent ev;
            ev.type = t;
            bridge.emit(ev);
        }

        ecnuvpn::LogEventBus::instance().unsubscribe(subscription);

        ok = expect(logs.size() == 12,
                    "lifecycle events should log and noisy events should not") && ok;
        for (const char* t : log_worthy_types) {
            const bool found = std::any_of(logs.begin(), logs.end(),
                [&](const ecnuvpn::TypedLogEvent& event) {
                    return event.component == "engine" && event.code == t;
                });
            ok = expect(found, (std::string("engine log emitted for ") + t).c_str()) && ok;
        }
        const bool noisy_logged = std::any_of(logs.begin(), logs.end(),
            [](const ecnuvpn::TypedLogEvent& event) {
                return event.code == "packet.inbound" ||
                       event.code == "packet.outbound" ||
                       event.code.rfind("dpd.", 0) == 0;
            });
        ok = expect(!noisy_logged, "packet and DPD events should not log") && ok;
    }

    return ok ? 0 : 1;
}
