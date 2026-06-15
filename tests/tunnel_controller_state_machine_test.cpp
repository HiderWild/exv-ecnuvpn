// TunnelController state machine test.
//
// This test exercises the production TunnelController directly.  It is kept
// narrow so the state-machine contract is covered without mirroring the
// implementation in a second standalone controller.

#include "core/tunnel_controller.hpp"
#include "core/tunnel_events.hpp"
#include "core/tunnel_intent.hpp"
#include "core/tunnel_state.hpp"
#include "support/fake_helper.hpp"
#include "support/fake_platform_network_ops.hpp"

#include <iostream>
#include <memory>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

exv::core::UserIntent make_intent(bool desired = true,
                                  bool auto_reconnect = true,
                                  const std::string &profile = "default") {
  exv::core::UserIntent intent;
  intent.desired_connected = desired;
  intent.auto_reconnect = auto_reconnect;
  intent.profile_id.value = profile;
  return intent;
}

struct ControllerFixture {
  std::shared_ptr<exv::test::FakeHelper> helper =
      std::make_shared<exv::test::FakeHelper>();
  std::shared_ptr<exv::test::FakePlatformNetworkOps> net_ops =
      std::make_shared<exv::test::FakePlatformNetworkOps>();
  exv::core::TunnelController controller{helper, net_ops};
  std::vector<exv::core::TunnelStatusSnapshot> snapshots;

  ControllerFixture() {
    controller.set_status_callback(
        [this](const exv::core::TunnelStatusSnapshot &snapshot) {
          snapshots.push_back(snapshot);
        });
  }
};

bool any_phase(const std::vector<exv::core::TunnelStatusSnapshot> &snapshots,
               exv::core::TunnelPhase phase) {
  for (const auto &snapshot : snapshots) {
    if (snapshot.phase == phase) {
      return true;
    }
  }
  return false;
}

bool phases_in_order(
    const std::vector<exv::core::TunnelStatusSnapshot> &snapshots,
    const std::vector<exv::core::TunnelPhase> &expected) {
  std::size_t index = 0;
  for (const auto &snapshot : snapshots) {
    if (index < expected.size() && snapshot.phase == expected[index]) {
      ++index;
    }
  }
  return index == expected.size();
}

bool test_connect_uses_production_state_machine() {
  using exv::core::TunnelPhase;

  ControllerFixture fixture;
  bool ok = true;
  ok = expect(fixture.controller.phase() == TunnelPhase::Idle,
              "initial production controller phase must be Idle") &&
       ok;

  fixture.controller.connect(make_intent(true));

  ok = expect(fixture.controller.phase() == TunnelPhase::Connected,
              "connect() must drive the fallback production flow to Connected") &&
       ok;
  ok = expect(phases_in_order(fixture.snapshots,
                              {TunnelPhase::PreparingHelper,
                               TunnelPhase::Authenticating,
                               TunnelPhase::ConnectingCstp,
                               TunnelPhase::OpeningPacketDevice,
                               TunnelPhase::ApplyingNetworkConfig,
                               TunnelPhase::Connected}),
              "connect() callbacks must expose the production phase order") &&
       ok;
  ok = expect(fixture.helper->hello_count() == 1,
              "production controller must send Hello before StartSession") &&
       ok;
  ok = expect(fixture.net_ops->prepare_count() == 1,
              "production controller must prepare the tunnel device") &&
       ok;
  ok = expect(fixture.net_ops->apply_count() == 1,
              "production controller must apply the tunnel config") &&
       ok;
  return ok;
}

bool test_disconnect_uses_cleanup_path() {
  using exv::core::TunnelPhase;

  ControllerFixture fixture;
  bool ok = true;
  fixture.controller.connect(make_intent(true));
  fixture.snapshots.clear();

  fixture.controller.disconnect();

  ok = expect(fixture.controller.phase() == TunnelPhase::Idle,
              "disconnect() must complete cleanup and return to Idle") &&
       ok;
  ok = expect(any_phase(fixture.snapshots, TunnelPhase::Disconnecting),
              "disconnect() must publish Disconnecting") &&
       ok;
  ok = expect(any_phase(fixture.snapshots, TunnelPhase::CleaningUp),
              "disconnect() must publish CleaningUp") &&
       ok;
  ok = expect(fixture.helper->shutdown_count() == 1,
              "disconnect() must actively shutdown the helper session") &&
       ok;
  return ok;
}

bool test_transport_closed_reconnects_only_when_enabled() {
  using exv::core::TunnelEventType;
  using exv::core::TunnelPhase;

  bool ok = true;
  {
    ControllerFixture fixture;
    fixture.controller.connect(make_intent(true, true));
    fixture.controller.on_event({TunnelEventType::TransportClosed});
    ok = expect(fixture.controller.phase() == TunnelPhase::Reconnecting,
                "TransportClosed with auto reconnect must enter Reconnecting") &&
         ok;
    fixture.controller.on_event({TunnelEventType::ReconnectTimerFired});
    ok = expect(fixture.controller.phase() == TunnelPhase::Authenticating,
                "ReconnectTimerFired must retry from Authenticating") &&
         ok;
  }
  {
    ControllerFixture fixture;
    fixture.controller.connect(make_intent(true, false));
    fixture.controller.on_event({TunnelEventType::TransportClosed});
    ok = expect(fixture.controller.phase() == TunnelPhase::Failed,
                "TransportClosed without auto reconnect must fail") &&
         ok;
    ok = expect(fixture.controller.status().last_error.has_value(),
                "failed transport close must expose last_error") &&
         ok;
  }
  return ok;
}

bool test_startup_failures_are_production_failures() {
  using exv::core::TunnelPhase;

  bool ok = true;
  {
    ControllerFixture fixture;
    fixture.helper->set_start_session_fail(true);
    fixture.controller.connect(make_intent(true));
    ok = expect(fixture.controller.phase() == TunnelPhase::Failed,
                "empty StartSession response must fail the controller") &&
         ok;
    ok = expect(fixture.net_ops->prepare_count() == 0,
                "network ops must not run after StartSession failure") &&
         ok;
  }
  {
    ControllerFixture fixture;
    fixture.net_ops->set_apply_should_fail(true);
    fixture.controller.connect(make_intent(true));
    ok = expect(fixture.controller.phase() == TunnelPhase::Failed,
                "apply_tunnel_config failure must fail the controller") &&
         ok;
    ok = expect(fixture.controller.status().last_error.has_value(),
                "apply_tunnel_config failure must expose last_error") &&
         ok;
  }
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = test_connect_uses_production_state_machine() && ok;
  ok = test_disconnect_uses_cleanup_path() && ok;
  ok = test_transport_closed_reconnects_only_when_enabled() && ok;
  ok = test_startup_failures_are_production_failures() && ok;

  if (ok) {
    std::cout << "tunnel_controller_state_machine_test: all assertions passed\n";
  } else {
    std::cerr
        << "tunnel_controller_state_machine_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
