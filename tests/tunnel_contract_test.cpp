// Tunnel controller manifest and generated C++ contract consistency test.

#include "contracts/generated/system_contract.hpp"
#include "core/tunnel_controller/tunnel_events.hpp"
#include "core/tunnel_controller/tunnel_intent.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"

#include <iostream>
#include <string_view>

namespace {

bool expect(bool condition, std::string_view message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

const exv::contracts::generated::TunnelPhaseContract *
find_phase(std::string_view name) {
  for (const auto &phase :
       exv::contracts::generated::TUNNEL_PHASE_CONTRACTS) {
    if (phase.name == name) {
      return &phase;
    }
  }
  return nullptr;
}

bool check_phase(std::string_view name, std::string_view wire_name,
                 exv::core::TunnelPhase, bool running, bool connected,
                 bool network_ready) {
  const auto *phase = find_phase(name);
  if (!phase) {
    std::cerr << "EXPECT FAILED: missing tunnel phase " << name << '\n';
    return false;
  }
  bool ok = true;
  ok = expect(phase->wire_name == wire_name,
              "tunnel phase wire name must match manifest") &&
       ok;
  ok = expect(phase->running == running,
              "tunnel phase running trait must match manifest") &&
       ok;
  ok = expect(phase->connected == connected,
              "tunnel phase connected trait must match manifest") &&
       ok;
  ok = expect(phase->network_ready == network_ready,
              "tunnel phase network_ready trait must match manifest") &&
       ok;
  return ok;
}

bool check_event(std::string_view name, exv::core::TunnelEventType) {
  return expect(exv::contracts::generated::is_tunnel_event(name),
                "manifest must declare every TunnelEventType");
}

bool check_disconnect_reason(std::string_view name,
                             exv::core::DisconnectReason) {
  return expect(exv::contracts::generated::is_tunnel_disconnect_reason(name),
                "manifest must declare every DisconnectReason");
}

} // namespace

int main() {
  bool ok = true;

  ok = expect(exv::contracts::generated::TUNNEL_PHASE_CONTRACTS.size() == 11,
              "manifest must declare every TunnelPhase") &&
       ok;
  ok = check_phase("Idle", "idle", exv::core::TunnelPhase::Idle, false, false,
                   false) &&
       ok;
  ok = check_phase("PreparingHelper", "preparing_helper",
                   exv::core::TunnelPhase::PreparingHelper, true, false,
                   false) &&
       ok;
  ok = check_phase("Authenticating", "authenticating",
                   exv::core::TunnelPhase::Authenticating, true, false,
                   false) &&
       ok;
  ok = check_phase("ConnectingCstp", "connecting_cstp",
                   exv::core::TunnelPhase::ConnectingCstp, true, false,
                   false) &&
       ok;
  ok = check_phase("ApplyingNetworkConfig", "applying_network_config",
                   exv::core::TunnelPhase::ApplyingNetworkConfig, true, false,
                   false) &&
       ok;
  ok = check_phase("OpeningPacketDevice", "opening_packet_device",
                   exv::core::TunnelPhase::OpeningPacketDevice, true, false,
                   false) &&
       ok;
  ok = check_phase("Connected", "connected", exv::core::TunnelPhase::Connected,
                   true, true, true) &&
       ok;
  ok = check_phase("Reconnecting", "reconnecting",
                   exv::core::TunnelPhase::Reconnecting, true, false, false) &&
       ok;
  ok = check_phase("Disconnecting", "disconnecting",
                   exv::core::TunnelPhase::Disconnecting, true, false,
                   false) &&
       ok;
  ok = check_phase("CleaningUp", "cleaning_up",
                   exv::core::TunnelPhase::CleaningUp, true, false, false) &&
       ok;
  ok = check_phase("Failed", "failed", exv::core::TunnelPhase::Failed, false,
                   false, false) &&
       ok;

  ok = check_event("UserConnect", exv::core::TunnelEventType::UserConnect) && ok;
  ok = check_event("UserDisconnect",
                   exv::core::TunnelEventType::UserDisconnect) &&
       ok;
  ok = check_event("SetAutoReconnect",
                   exv::core::TunnelEventType::SetAutoReconnect) &&
       ok;
  ok = check_event("HelperReady", exv::core::TunnelEventType::HelperReady) &&
       ok;
  ok = check_event("AuthSucceeded", exv::core::TunnelEventType::AuthSucceeded) &&
       ok;
  ok = check_event("AuthFailed", exv::core::TunnelEventType::AuthFailed) && ok;
  ok = check_event("CstpConnected",
                   exv::core::TunnelEventType::CstpConnected) &&
       ok;
  ok = check_event("NetworkConfigApplied",
                   exv::core::TunnelEventType::NetworkConfigApplied) &&
       ok;
  ok = check_event("PacketLoopStarted",
                   exv::core::TunnelEventType::PacketLoopStarted) &&
       ok;
  ok = check_event("TransportClosed",
                   exv::core::TunnelEventType::TransportClosed) &&
       ok;
  ok = check_event("PacketDeviceFailed",
                   exv::core::TunnelEventType::PacketDeviceFailed) &&
       ok;
  ok = check_event("HelperLost", exv::core::TunnelEventType::HelperLost) && ok;
  ok = check_event("LeaseExpired", exv::core::TunnelEventType::LeaseExpired) &&
       ok;
  ok = check_event("ReconnectTimerFired",
                   exv::core::TunnelEventType::ReconnectTimerFired) &&
       ok;
  ok = check_event("CleanupSucceeded",
                   exv::core::TunnelEventType::CleanupSucceeded) &&
       ok;
  ok = check_event("CleanupFailed",
                   exv::core::TunnelEventType::CleanupFailed) &&
       ok;

  ok = check_disconnect_reason("UserRequested",
                               exv::core::DisconnectReason::UserRequested) &&
       ok;
  ok = check_disconnect_reason("AuthFailed",
                               exv::core::DisconnectReason::AuthFailed) &&
       ok;
  ok = check_disconnect_reason("CertError",
                               exv::core::DisconnectReason::CertError) &&
       ok;
  ok = check_disconnect_reason("TransportClosed",
                               exv::core::DisconnectReason::TransportClosed) &&
       ok;
  ok = check_disconnect_reason("HelperLost",
                               exv::core::DisconnectReason::HelperLost) &&
       ok;
  ok = check_disconnect_reason(
           "PacketDeviceFailed",
           exv::core::DisconnectReason::PacketDeviceFailed) &&
       ok;
  ok = check_disconnect_reason(
           "NetworkConfigFailed",
           exv::core::DisconnectReason::NetworkConfigFailed) &&
       ok;
  ok = check_disconnect_reason("LeaseExpired",
                               exv::core::DisconnectReason::LeaseExpired) &&
       ok;
  ok = expect(exv::contracts::generated::is_tunnel_error_domain("transport"),
              "manifest must declare transport error domain") &&
       ok;
  ok = expect(exv::contracts::generated::is_tunnel_error_domain("packet"),
              "manifest must declare packet error domain") &&
       ok;

  if (!ok) {
    std::cerr << "tunnel_contract_test: FAILED\n";
    return 1;
  }
  std::cout << "tunnel_contract_test: all assertions passed\n";
  return 0;
}
