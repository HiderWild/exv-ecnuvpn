#pragma once

#include <nlohmann/json.hpp>

#include <string>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {

// Stable session lifecycle phases for the native engine.
//
// JSON encoding: session_phase_to_json() returns a string with the same name as
// the enumerator (e.g. "idle", "packet_loop").
enum class SessionPhase {
  idle,
  authenticating,
  authenticated,
  configuring_network,
  packet_loop,
  reconnecting,
  stopping,
  stopped,
  failed,
};

struct TunnelMetadata {
  std::string interface_name;
  int interface_index = -1;
  std::string internal_ip4_address;
  std::string internal_ip4_netmask;
  int mtu = 1290;
  std::vector<std::string> routes;
  std::vector<std::string> server_bypass_ips;
  std::string dtls_state = "disabled";
  std::string dtls_fallback_reason;
};

// SessionState is a clean-room state model used by the native engine boundary.
//
// Stable JSON schema (session_state_to_json):
// - phase: string (see SessionPhase)
// - tunnel_ready: bool
// - packet_loop_ready: bool
// - network_ready: bool (true only when phase == packet_loop AND both ready)
// - tunnel_metadata: object (see tunnel_metadata_to_json)
// - last_event_message: string (UTF-8 preserved as-is)
// - failure: { code: string, message: string }
//
// Notes:
// - No OpenConnect process/pid field exists or is required.
struct SessionState {
  SessionPhase phase = SessionPhase::idle;

  TunnelMetadata tunnel;
  bool tunnel_ready = false;
  bool packet_loop_ready = false;

  std::string last_event_message;

  std::string failure_code;
  std::string failure_message;

  bool network_ready() const;

  void auth_started();
  void auth_succeeded();
  void tunnel_configured(const TunnelMetadata &metadata);
  void packet_loop_started();
  void stopped();
  void failed(const std::string &code, const std::string &message);
};

nlohmann::json session_phase_to_json(SessionPhase phase);
nlohmann::json tunnel_metadata_to_json(const TunnelMetadata &metadata);
nlohmann::json session_state_to_json(const SessionState &state);

} // namespace vpn_engine
} // namespace ecnuvpn
