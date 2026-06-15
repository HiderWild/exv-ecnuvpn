#include <iostream>
#include <memory>
#include <string_view>

import exv.core.tunnel;

namespace {

bool expect(bool condition, std::string_view message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

} // namespace

int main() {
  namespace tunnel = exv::core::tunnel;

  bool ok = true;

  tunnel::TunnelController *controller = nullptr;
  ok = expect(controller == nullptr,
              "public tunnel module must export the controller type") &&
       ok;

  tunnel::UserIntent intent;
  intent.desired_connected = true;
  intent.auto_reconnect = true;
  intent.profile_id.value = "module-profile";
  ok = expect(intent.profile_id.value == "module-profile",
              "public tunnel module must export user intent types") &&
       ok;

  tunnel::TunnelStatusSnapshot snapshot;
  snapshot.phase = tunnel::TunnelPhase::Idle;
  snapshot.desired_connected = intent.desired_connected;
  ok = expect(snapshot.phase == tunnel::TunnelPhase::Idle,
              "public tunnel module must export status snapshot types") &&
       ok;

  tunnel::TunnelEvent event{tunnel::TunnelEventType::HelperReady};
  ok = expect(event.type == tunnel::TunnelEventType::HelperReady,
              "public tunnel module must export event types") &&
       ok;

  tunnel::ReconnectConfig reconnect_config;
  reconnect_config.max_attempts = 3;
  ok = expect(reconnect_config.max_attempts == 3,
              "public tunnel module must export reconnect configuration") &&
       ok;

  if (!ok) {
    std::cerr << "tunnel_public_module_smoke_test: FAILED\n";
    return 1;
  }
  std::cout << "tunnel_public_module_smoke_test: all assertions passed\n";
  return 0;
}
