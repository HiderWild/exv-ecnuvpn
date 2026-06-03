// Integration test: helper lost / IPC disconnect behavior.
//
// Verifies that helper disconnection during various phases produces
// the correct error domain, error code, and state transitions.

#include "core/tunnel_intent.hpp"
#include "core/tunnel_state.hpp"
#include "core/tunnel_events.hpp"
#include "core/reconnect_policy.hpp"
#include "support/fake_helper.hpp"
#include "support/fake_platform_network_ops.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

// Standalone state machine mirroring TunnelController behavior.
// Includes HelperLost event handling.
class TestTunnelController {
public:
    using StatusCallback = std::function<void(const exv::core::TunnelStatusSnapshot&)>;

    void connect(exv::core::UserIntent intent) {
        intent_ = std::move(intent);
        intent_.desired_connected = true;
        if (phase_ == exv::core::TunnelPhase::Idle ||
            phase_ == exv::core::TunnelPhase::Failed) {
            attempt_count_ = 0;
            transition(exv::core::TunnelPhase::PreparingHelper);
        }
    }

    void disconnect(exv::core::DisconnectReason reason =
                        exv::core::DisconnectReason::UserRequested) {
        if (phase_ != exv::core::TunnelPhase::Idle &&
            phase_ != exv::core::TunnelPhase::Disconnecting) {
            intent_.desired_connected = false;
            intent_.user_disconnect_reason = reason;
            transition(exv::core::TunnelPhase::Disconnecting);
        }
    }

    void set_auto_reconnect(bool enabled) { auto_reconnect_ = enabled; }

    exv::core::TunnelPhase phase() const { return phase_; }

    exv::core::TunnelStatusSnapshot status() const {
        exv::core::TunnelStatusSnapshot snap;
        snap.phase = phase_;
        snap.desired_connected = intent_.desired_connected;
        snap.auto_reconnect = auto_reconnect_;
        snap.reconnect = exv::core::ReconnectInfo{attempt_count_, 0};
        snap.last_error = last_error_;
        return snap;
    }

    void on_event(exv::core::TunnelEvent event) {
        using exv::core::TunnelPhase;
        using exv::core::TunnelEventType;

        switch (event.type) {
            case TunnelEventType::HelperReady:
                if (phase_ == TunnelPhase::PreparingHelper)
                    transition(TunnelPhase::Authenticating);
                break;
            case TunnelEventType::AuthSucceeded:
                if (phase_ == TunnelPhase::Authenticating)
                    transition(TunnelPhase::ConnectingCstp);
                break;
            case TunnelEventType::AuthFailed:
                if (phase_ == TunnelPhase::Authenticating) {
                    last_error_ = exv::core::ErrorInfo{
                        "auth", "auth_failed",
                        "Authentication failed", {}, "",
                        false, "Check credentials"
                    };
                    transition(TunnelPhase::Failed);
                }
                break;
            case TunnelEventType::CstpConnected:
                if (phase_ == TunnelPhase::ConnectingCstp)
                    transition(TunnelPhase::ApplyingNetworkConfig);
                break;
            case TunnelEventType::NetworkConfigApplied:
                if (phase_ == TunnelPhase::ApplyingNetworkConfig)
                    transition(TunnelPhase::OpeningPacketDevice);
                break;
            case TunnelEventType::PacketLoopStarted:
                if (phase_ == TunnelPhase::OpeningPacketDevice) {
                    attempt_count_ = 0;
                    transition(TunnelPhase::Connected);
                }
                break;
            case TunnelEventType::TransportClosed:
                if (phase_ == TunnelPhase::Connected) {
                    if (auto_reconnect_ && intent_.desired_connected) {
                        attempt_count_++;
                        transition(TunnelPhase::Reconnecting);
                    } else {
                        transition(TunnelPhase::Failed);
                    }
                }
                break;
            case TunnelEventType::HelperLost:
                // Helper lost is always a terminal failure
                last_error_ = exv::core::ErrorInfo{
                    "helper", "helper_unavailable",
                    "Helper process disconnected unexpectedly", {}, "",
                    false, "Restart helper"
                };
                if (phase_ != TunnelPhase::Idle &&
                    phase_ != TunnelPhase::Failed) {
                    transition(TunnelPhase::Failed);
                }
                break;
            case TunnelEventType::ReconnectTimerFired:
                if (phase_ == TunnelPhase::Reconnecting)
                    transition(TunnelPhase::Authenticating);
                break;
            case TunnelEventType::CleanupSucceeded:
            case TunnelEventType::CleanupFailed:
                if (phase_ == TunnelPhase::Disconnecting)
                    transition(TunnelPhase::Idle);
                break;
            default:
                break;
        }
    }

    void set_status_callback(StatusCallback cb) { status_cb_ = std::move(cb); }

private:
    void transition(exv::core::TunnelPhase new_phase) {
        phase_ = new_phase;
        if (status_cb_) status_cb_(status());
    }

    exv::core::TunnelPhase phase_ = exv::core::TunnelPhase::Idle;
    exv::core::UserIntent intent_;
    bool auto_reconnect_ = true;
    int attempt_count_ = 0;
    std::optional<exv::core::ErrorInfo> last_error_;
    StatusCallback status_cb_;
};

// Drive controller through the full connect sequence to Connected
static void drive_to_connected(TestTunnelController& ctrl) {
    using exv::core::TunnelEventType;
    ctrl.on_event({TunnelEventType::HelperReady});
    ctrl.on_event({TunnelEventType::AuthSucceeded});
    ctrl.on_event({TunnelEventType::CstpConnected});
    ctrl.on_event({TunnelEventType::NetworkConfigApplied});
    ctrl.on_event({TunnelEventType::PacketLoopStarted});
}

} // namespace

int main() {
    bool ok = true;

    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;
    using exv::core::UserIntent;

    // === Test: helper_lost_while_connected ===
    // Drive to Connected, simulate helper disconnect, verify Failed
    {
        TestTunnelController ctrl;

        std::vector<TunnelPhase> observed_phases;
        ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
            observed_phases.push_back(snap.phase);
        });

        UserIntent intent;
        intent.desired_connected = true;
        intent.auto_reconnect = true;
        ctrl.connect(intent);
        drive_to_connected(ctrl);

        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "should be Connected before helper lost") && ok;

        // Simulate helper disconnect
        ctrl.on_event({TunnelEventType::HelperLost});

        // Verify: transitions to Failed
        ok = expect(ctrl.phase() == TunnelPhase::Failed,
                    "helper lost while connected should transition to Failed") && ok;

        // Verify: error domain is "helper"
        auto snap = ctrl.status();
        ok = expect(snap.last_error.has_value(),
                    "status should include last_error after helper lost") && ok;
        if (snap.last_error.has_value()) {
            ok = expect(snap.last_error->domain == "helper",
                        "error domain should be 'helper'") && ok;
            ok = expect(snap.last_error->code == "helper_unavailable",
                        "error code should be 'helper_unavailable'") && ok;
            ok = expect(!snap.last_error->recoverable,
                        "helper lost should be non-recoverable") && ok;
        }
    }

    // === Test: helper_lost_during_applying_config ===
    // Drive to ApplyingNetworkConfig, simulate helper disconnect, verify Failed
    {
        TestTunnelController ctrl;

        UserIntent intent;
        intent.desired_connected = true;
        intent.auto_reconnect = true;
        ctrl.connect(intent);

        // Drive to ApplyingNetworkConfig
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ok = expect(ctrl.phase() == TunnelPhase::ApplyingNetworkConfig,
                    "should be ApplyingNetworkConfig") && ok;

        // Simulate helper disconnect
        ctrl.on_event({TunnelEventType::HelperLost});

        // Verify: transitions to Failed
        ok = expect(ctrl.phase() == TunnelPhase::Failed,
                    "helper lost during ApplyingNetworkConfig should transition to Failed") && ok;

        // Verify: error is not recoverable
        auto snap = ctrl.status();
        ok = expect(snap.last_error.has_value(),
                    "status should include last_error") && ok;
        if (snap.last_error.has_value()) {
            ok = expect(!snap.last_error->recoverable,
                        "helper lost during config should be non-recoverable") && ok;
            ok = expect(snap.last_error->domain == "helper",
                        "error domain should be 'helper'") && ok;
            ok = expect(snap.last_error->code == "helper_unavailable",
                        "error code should be 'helper_unavailable'") && ok;
        }
    }

    // === Test: helper_lost_during_authenticating ===
    {
        TestTunnelController ctrl;

        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);

        ctrl.on_event({TunnelEventType::HelperReady});
        ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                    "should be Authenticating") && ok;

        ctrl.on_event({TunnelEventType::HelperLost});
        ok = expect(ctrl.phase() == TunnelPhase::Failed,
                    "helper lost during Authenticating should transition to Failed") && ok;
    }

    // === Test: FakeHelper simulate_ipc_lost ===
    {
        auto helper = std::make_shared<exv::test::FakeHelper>();
        helper->connect();
        ok = expect(helper->is_connected(), "helper should be connected") && ok;

        bool callback_fired = false;
        helper->set_disconnect_callback([&]() { callback_fired = true; });

        helper->simulate_ipc_lost();
        ok = expect(!helper->is_connected(),
                    "helper should be disconnected after ipc_lost") && ok;
        ok = expect(callback_fired,
                    "disconnect callback should fire on ipc_lost") && ok;
        ok = expect(helper->ipc_lost(),
                    "ipc_lost flag should be set") && ok;
    }

    // === Test: FakeHelper simulate_disconnect vs simulate_ipc_lost ===
    {
        auto helper = std::make_shared<exv::test::FakeHelper>();
        helper->connect();

        // simulate_disconnect does NOT set ipc_lost flag
        helper->simulate_disconnect();
        ok = expect(!helper->is_connected(),
                    "should be disconnected after simulate_disconnect") && ok;
        ok = expect(!helper->ipc_lost(),
                    "ipc_lost should NOT be set after simulate_disconnect") && ok;

        // Reconnect and test simulate_ipc_lost
        helper->connect();
        helper->simulate_ipc_lost();
        ok = expect(helper->ipc_lost(),
                    "ipc_lost should be set after simulate_ipc_lost") && ok;
    }

    // === Test: FakePlatformNetworkOps adapter_create_fail ===
    {
        auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
        net_ops->set_adapter_create_fail(true);

        auto desc = net_ops->prepare_tunnel_device("test-adapter");
        ok = expect(!desc.is_open,
                    "prepare_tunnel_device with adapter_create_fail should return closed device") && ok;
    }

    // === Test: FakePlatformNetworkOps unsupported ===
    {
        auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
        net_ops->set_unsupported(true);

        auto desc = net_ops->prepare_tunnel_device("test-adapter");
        ok = expect(!desc.is_open,
                    "prepare_tunnel_device with unsupported should return closed device") && ok;
    }

    // === Test: FakePlatformNetworkOps route_add_fail ===
    {
        auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();

        // First prepare a device
        auto desc = net_ops->prepare_tunnel_device("test-adapter");
        ok = expect(desc.is_open, "device should be open") && ok;

        // Now make route operations fail
        net_ops->set_route_add_fail(true);
        exv::platform::TunnelConfig config;
        bool result = net_ops->apply_tunnel_config(desc, config);
        ok = expect(!result,
                    "apply_tunnel_config with route_add_fail should return false") && ok;
    }

    // === Test: FakePlatformNetworkOps dns_fail ===
    {
        auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();

        auto desc = net_ops->prepare_tunnel_device("test-adapter");
        net_ops->set_dns_fail(true);
        exv::platform::TunnelConfig config;
        bool result = net_ops->apply_tunnel_config(desc, config);
        ok = expect(!result,
                    "apply_tunnel_config with dns_fail should return false") && ok;
    }

    // === Test: helper lost while Reconnecting also goes to Failed ===
    {
        TestTunnelController ctrl;

        UserIntent intent;
        intent.desired_connected = true;
        intent.auto_reconnect = true;
        ctrl.connect(intent);
        drive_to_connected(ctrl);

        // Transport drops, enters Reconnecting
        ctrl.on_event({TunnelEventType::TransportClosed});
        ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                    "should be Reconnecting after TransportClosed") && ok;

        // Helper lost during reconnect
        ctrl.on_event({TunnelEventType::HelperLost});
        ok = expect(ctrl.phase() == TunnelPhase::Failed,
                    "helper lost during Reconnecting should go to Failed") && ok;
    }

    if (ok) {
        std::cout << "helper_lost_test: all assertions passed\n";
    } else {
        std::cerr << "helper_lost_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
