#include "core/tunnel_controller/engine_event_bridge.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"

#include <utility>
#include <vector>

namespace exv::core {
namespace {

bool is_noisy_event(const std::string& type) {
    return type == "packet.inbound" ||
           type == "packet.outbound" ||
           type.rfind("dpd.", 0) == 0;
}

bool is_log_worthy_event(const std::string& type) {
    if (is_noisy_event(type)) {
        return false;
    }
    return type == "native.starting" ||
           type == "auth.started" ||
           type == "auth.succeeded" ||
           type == "auth.failed" ||
           type == "cstp.connected" ||
           type == "cstp.failed" ||
           type == "packet.loop.started" ||
           type == "packet_device.failed" ||
           type == "transport.closed" ||
           type == "packet.loop.stopped" ||
           type.rfind("reconnect.", 0) == 0;
}

std::string log_level_for_event(const ecnuvpn::vpn_engine::VpnEngineEvent& event) {
    if (event.level == "error" || event.type.find(".failed") != std::string::npos) {
        return "ERROR";
    }
    if (event.level == "warn" || event.level == "warning") {
        return "WARN";
    }
    return "INFO";
}

void log_engine_event(const ecnuvpn::vpn_engine::VpnEngineEvent& event) {
    if (!is_log_worthy_event(event.type)) {
        return;
    }

    std::vector<std::pair<std::string, std::string>> fields;
    fields.reserve(event.fields.size());
    for (const auto& field : event.fields) {
        fields.emplace_back(field.first, field.second);
    }

    exv::observability::LogFacade::event(log_level_for_event(event), "engine", event.type,
                           event.message.empty() ? event.type : event.message,
                           fields);
}

} // namespace

EngineEventBridge::EngineEventBridge(EventCallback callback)
    : callback_(std::move(callback)) {}

EngineEventBridge::~EngineEventBridge() = default;

void EngineEventBridge::emit(const ecnuvpn::vpn_engine::VpnEngineEvent& event) {
    log_engine_event(event);

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
