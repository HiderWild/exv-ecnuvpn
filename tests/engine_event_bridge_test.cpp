#include "core/tunnel_controller/engine_event_bridge.hpp"
#include "observability/log_facade.hpp"
#include "observability/log_sink.hpp"
#include "observability/log_service.hpp"

#include <algorithm>
#include <iostream>
#include <memory>
#include <mutex>
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

class CapturingLogSink final : public exv::observability::LogSink {
public:
    void write(const exv::observability::LogEvent& event) override {
        std::lock_guard<std::mutex> lock(mutex_);
        logs_.push_back(event);
    }

    std::vector<exv::observability::LogEvent> logs() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return logs_;
    }

private:
    mutable std::mutex mutex_;
    std::vector<exv::observability::LogEvent> logs_;
};

std::shared_ptr<CapturingLogSink> install_capturing_log_sink() {
    auto sink = std::make_shared<CapturingLogSink>();
    auto service = std::make_shared<exv::observability::LogService>();
    service->add_sink(sink);
    exv::observability::LogFacade::configure(service);
    return sink;
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
        ok = expect(exv::core::EngineEventBridge::map_event("auth.challenge_required", &out) &&
                    out == events::TunnelEventType::AuthChallengeRequired,
                    "auth.challenge_required -> AuthChallengeRequired") && ok;
    }
    {
        events::TunnelEventType out{};
        ok = expect(exv::core::EngineEventBridge::map_event("auth.group_required", &out) &&
                    out == events::TunnelEventType::AuthGroupRequired,
                    "auth.group_required -> AuthGroupRequired") && ok;
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

        exv::vpn_engine::VpnEngineEvent ev1;
        ev1.type = "auth.succeeded";
        bridge.emit(ev1);

        exv::vpn_engine::VpnEngineEvent ev2;
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

        exv::vpn_engine::VpnEngineEvent ev;
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
            "auth.challenge_required",
            "auth.group_required",
            "cstp.connected",
            "packet.loop.started",
            "transport.closed",
            "packet_device.failed"
        };
        for (const char* t : types) {
            exv::vpn_engine::VpnEngineEvent ev;
            ev.type = t;
            bridge.emit(ev);
        }

        ok = expect(received.size() == 8, "all 8 mapped types received") && ok;
        ok = expect(received[0] == events::TunnelEventType::AuthSucceeded,
                    "index 0: AuthSucceeded") && ok;
        ok = expect(received[1] == events::TunnelEventType::AuthFailed,
                    "index 1: AuthFailed") && ok;
        ok = expect(received[2] == events::TunnelEventType::AuthChallengeRequired,
                    "index 2: AuthChallengeRequired") && ok;
        ok = expect(received[3] == events::TunnelEventType::AuthGroupRequired,
                    "index 3: AuthGroupRequired") && ok;
        ok = expect(received[4] == events::TunnelEventType::CstpConnected,
                    "index 4: CstpConnected") && ok;
        ok = expect(received[5] == events::TunnelEventType::PacketLoopStarted,
                    "index 5: PacketLoopStarted") && ok;
        ok = expect(received[6] == events::TunnelEventType::TransportClosed,
                    "index 6: TransportClosed") && ok;
        ok = expect(received[7] == events::TunnelEventType::PacketDeviceFailed,
                    "index 7: PacketDeviceFailed") && ok;
    }

    // ---------------------------------------------------------------
    // 6. mapped failure events preserve native code/message payloads.
    // ---------------------------------------------------------------

    {
        std::vector<events::TunnelEvent> received;
        exv::core::EngineEventBridge bridge(
            [&](events::TunnelEvent e) { received.push_back(e); });

        exv::vpn_engine::VpnEngineEvent ev;
        ev.type = "auth.failed";
        ev.message = "SAML authentication is required";
        ev.fields.emplace("code", "saml_required_unsupported");
        ev.fields.emplace("recoverable", "false");
        bridge.emit(ev);

        ok = expect(received.size() == 1,
                    "auth.failed should emit one tunnel event") && ok;
        ok = expect(received[0].type == events::TunnelEventType::AuthFailed,
                    "auth.failed event type should be preserved") && ok;
        ok = expect(received[0].code == "saml_required_unsupported",
                    "native event code should be preserved") && ok;
        ok = expect(received[0].message == "SAML authentication is required",
                    "native event message should be preserved") && ok;
        ok = expect(!received[0].recoverable,
                    "native event recoverable flag should be parsed") && ok;
    }

    // ---------------------------------------------------------------
    // 7. emit(): null callback does not crash
    // ---------------------------------------------------------------

    {
        exv::core::EngineEventBridge bridge(nullptr);
        exv::vpn_engine::VpnEngineEvent ev;
        ev.type = "auth.succeeded";
        bridge.emit(ev);  // Should not crash
        ok = expect(true, "null callback does not crash") && ok;
    }

    // ---------------------------------------------------------------
    // 8. emit(): lifecycle events publish user-facing engine logs,
    //    while packet/DPD noise is filtered out.
    // ---------------------------------------------------------------

    {
        auto log_sink = install_capturing_log_sink();

        exv::core::EngineEventBridge bridge(nullptr);
        const char* log_worthy_types[] = {
            "native.starting",
            "auth.started",
            "auth.succeeded",
            "auth.failed",
            "auth.challenge_required",
            "auth.group_required",
            "csd.required_unsupported",
            "dtls.unavailable",
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
            exv::vpn_engine::VpnEngineEvent ev;
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
            exv::vpn_engine::VpnEngineEvent ev;
            ev.type = t;
            bridge.emit(ev);
        }

        exv::observability::LogFacade::flush();
        auto logs = log_sink->logs();
        exv::observability::LogFacade::shutdown();

        ok = expect(logs.size() == 16,
                    "lifecycle events should log and noisy events should not") && ok;
        for (const char* t : log_worthy_types) {
            const bool found = std::any_of(logs.begin(), logs.end(),
                [&](const exv::observability::LogEvent& event) {
                    return event.component == "engine" && event.code == t;
                });
            ok = expect(found, (std::string("engine log emitted for ") + t).c_str()) && ok;
        }
        const bool noisy_logged = std::any_of(logs.begin(), logs.end(),
            [](const exv::observability::LogEvent& event) {
                return event.code == "packet.inbound" ||
                       event.code == "packet.outbound" ||
                       event.code.rfind("dpd.", 0) == 0;
            });
        ok = expect(!noisy_logged, "packet and DPD events should not log") && ok;
    }

    // ---------------------------------------------------------------
    // 9. emit(): engine logs redact secret-bearing message and fields.
    // ---------------------------------------------------------------

    {
        auto log_sink = install_capturing_log_sink();

        exv::core::EngineEventBridge bridge(nullptr);
        exv::vpn_engine::VpnEngineEvent ev;
        ev.type = "auth.failed";
        ev.level = "error";
        ev.message = "failed with password=SECRET_PASSWORD webvpn=SECRET_COOKIE";
        ev.fields.emplace("cookie", "webvpn=SECRET_COOKIE");
        ev.fields.emplace("auth_token", "SECRET_TOKEN");
        ev.fields.emplace("code", "auth_failed");
        bridge.emit(ev);

        exv::observability::LogFacade::flush();
        auto logs = log_sink->logs();
        exv::observability::LogFacade::shutdown();

        ok = expect(!logs.empty(), "auth.failed should log") && ok;
        std::string dumped;
        for (const auto& log : logs) {
            dumped += log.message;
            dumped += log.code;
            for (const auto& field : log.fields) {
                dumped += field.first;
                dumped += field.second;
            }
        }
        ok = expect(dumped.find("SECRET_PASSWORD") == std::string::npos,
                    "engine logs should redact password values") && ok;
        ok = expect(dumped.find("SECRET_COOKIE") == std::string::npos,
                    "engine logs should redact cookie values") && ok;
        ok = expect(dumped.find("SECRET_TOKEN") == std::string::npos,
                    "engine logs should redact token values") && ok;
    }

    return ok ? 0 : 1;
}
