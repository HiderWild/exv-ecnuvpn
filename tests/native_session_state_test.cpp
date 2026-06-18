#include "vpn_engine/session_state.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

} // namespace

int main() {
  bool ok = true;

  using ecnuvpn::vpn_engine::SessionPhase;
  using ecnuvpn::vpn_engine::SessionState;
  using ecnuvpn::vpn_engine::TunnelMetadata;

  SessionState state;
  ok = expect(state.phase == SessionPhase::idle,
              "initial session phase should be idle") &&
       ok;
  ok = expect(!state.network_ready(),
              "initial session should not be network-ready") &&
       ok;

  state.auth_started();
  ok = expect(state.phase == SessionPhase::authenticating,
              "auth_started should move session into authenticating") &&
       ok;

  state.auth_succeeded();
  ok = expect(state.phase == SessionPhase::authenticated,
              "auth_succeeded should move session into authenticated") &&
       ok;

  TunnelMetadata meta;
  meta.interface_name = "tun0";
  meta.interface_index = 7;
  meta.internal_ip4_address = "10.0.0.2";
  meta.internal_ip4_netmask = "255.255.255.255";
  meta.mtu = 1290;
  meta.routes = {"59.78.176.0/20"};
  meta.server_bypass_ips = {"1.2.3.4"};
  meta.dtls_state = "attempted_and_fell_back_to_tls";
  meta.dtls_fallback_reason = "native DTLS backend unavailable; using CSTP/TLS";

  state.tunnel_configured(meta);
  ok = expect(state.phase == SessionPhase::configuring_network,
              "tunnel_configured should move session into configuring_network") &&
       ok;
  ok = expect(state.tunnel_ready,
              "tunnel_configured should mark tunnel_ready") &&
       ok;
  ok = expect(!state.network_ready(),
              "network_ready must be false before packet loop starts") &&
       ok;

  state.packet_loop_started();
  ok = expect(state.phase == SessionPhase::packet_loop,
              "packet_loop_started should move session into packet_loop") &&
       ok;
  ok = expect(state.packet_loop_ready,
              "packet_loop_started should mark packet_loop_ready") &&
       ok;
  ok = expect(state.network_ready(),
              "network_ready must be true only after tunnel + packet loop are ready") &&
       ok;

  const std::string utf8_message = "连接成功";
  state.last_event_message = utf8_message;

  nlohmann::json j = ecnuvpn::vpn_engine::session_state_to_json(state);
  ok = expect(j.value("phase", std::string()) == "packet_loop",
              "session_state_to_json should expose stable phase strings") &&
       ok;
  ok = expect(j.value("network_ready", false),
              "session_state_to_json should expose network_ready") &&
       ok;
  ok = expect(j.value("last_event_message", std::string()) == "连接成功",
              "session_state_to_json should preserve UTF-8 event text") &&
       ok;
  ok = expect(j["tunnel_metadata"].value("dtls_state", std::string()) ==
                  "attempted_and_fell_back_to_tls",
              "session_state_to_json should expose native DTLS state") &&
       ok;
  ok = expect(j["tunnel_metadata"]
                  .value("dtls_fallback_reason", std::string())
                  .find("CSTP/TLS") != std::string::npos,
              "session_state_to_json should expose DTLS fallback reason") &&
       ok;
  ok = expect(!j.contains("pid"),
              "session_state JSON must not require an OpenConnect PID") &&
       ok;

  state.stopped();
  ok = expect(state.phase == SessionPhase::stopped,
              "stopped should move session into stopped") &&
       ok;
  ok = expect(!state.network_ready(),
              "stopped session should not be network-ready") &&
       ok;

  state.failed("transport_error", "socket closed");
  ok = expect(state.phase == SessionPhase::failed,
              "failed should move session into failed") &&
       ok;
  ok = expect(!state.network_ready(),
              "failed session should not be network-ready") &&
       ok;

  nlohmann::json failed_json = ecnuvpn::vpn_engine::session_state_to_json(state);
  ok = expect(failed_json["failure"].value("code", std::string()) ==
                  "transport_error",
              "failed session JSON should expose failure code") &&
       ok;

  return ok ? 0 : 1;
}
