#include "vpn_engine/session_state.hpp"

#include <utility>

namespace ecnuvpn {
namespace vpn_engine {

bool SessionState::network_ready() const {
  return phase == SessionPhase::packet_loop && tunnel_ready && packet_loop_ready;
}

void SessionState::auth_started() {
  phase = SessionPhase::authenticating;
  failure_code.clear();
  failure_message.clear();
}

void SessionState::auth_succeeded() {
  phase = SessionPhase::authenticated;
  failure_code.clear();
  failure_message.clear();
}

void SessionState::tunnel_configured(const TunnelMetadata &metadata) {
  tunnel = metadata;
  tunnel_ready = true;
  phase = SessionPhase::configuring_network;
}

void SessionState::packet_loop_started() {
  packet_loop_ready = true;
  phase = SessionPhase::packet_loop;
}

void SessionState::stopped() {
  phase = SessionPhase::stopped;
}

void SessionState::failed(const std::string &code, const std::string &message) {
  phase = SessionPhase::failed;
  failure_code = code;
  failure_message = message;
}

nlohmann::json session_phase_to_json(SessionPhase phase) {
  switch (phase) {
  case SessionPhase::idle:
    return "idle";
  case SessionPhase::authenticating:
    return "authenticating";
  case SessionPhase::authenticated:
    return "authenticated";
  case SessionPhase::configuring_network:
    return "configuring_network";
  case SessionPhase::packet_loop:
    return "packet_loop";
  case SessionPhase::reconnecting:
    return "reconnecting";
  case SessionPhase::stopping:
    return "stopping";
  case SessionPhase::stopped:
    return "stopped";
  case SessionPhase::failed:
    return "failed";
  }

  return "idle";
}

nlohmann::json tunnel_metadata_to_json(const TunnelMetadata &metadata) {
  return nlohmann::json{{"interface_name", metadata.interface_name},
                        {"interface_index", metadata.interface_index},
                        {"internal_ip4_address", metadata.internal_ip4_address},
                        {"internal_ip4_netmask", metadata.internal_ip4_netmask},
                        {"mtu", metadata.mtu},
                        {"routes", metadata.routes},
                        {"server_bypass_ips", metadata.server_bypass_ips},
                        {"dtls_state", metadata.dtls_state},
                        {"dtls_fallback_reason",
                         metadata.dtls_fallback_reason}};
}

nlohmann::json session_state_to_json(const SessionState &state) {
  return nlohmann::json{
      {"phase", session_phase_to_json(state.phase)},
      {"tunnel_ready", state.tunnel_ready},
      {"packet_loop_ready", state.packet_loop_ready},
      {"network_ready", state.network_ready()},
      {"tunnel_metadata", tunnel_metadata_to_json(state.tunnel)},
      {"last_event_message", state.last_event_message},
      {"failure",
       nlohmann::json{{"code", state.failure_code},
                     {"message", state.failure_message}}},
  };
}

} // namespace vpn_engine
} // namespace ecnuvpn
