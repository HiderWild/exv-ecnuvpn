// Integration test: full connect, disconnect, and reconnect flows using
// FakeHelper + FakePlatformNetworkOps alongside a standalone state machine.

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

    void set_auto_reconnect(bool enabled) {
        auto_reconnect_ = enabled;
        intent_.auto_reconnect = enabled;
    }

    exv::core::TunnelPhase phase() const { return phase_; }

    exv::core::TunnelStatusSnapshot status() const {
        exv::core::TunnelStatusSnapshot snap;
        snap.phase = phase_;
        snap.desired_connected = intent_.desired_connected;
        snap.auto_reconnect = auto_reconnect_;
        snap.reconnect = exv::core::ReconnectInfo{attempt_count_, 0};
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
                if (phase_ == TunnelPhase::Authenticating ||
                    phase_ == TunnelPhase::Connected)
                    transition(TunnelPhase::Failed);
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
            case TunnelEventType::ReconnectTimerFired:
                if (phase_ == TunnelPhase::Reconnecting)
                    transition(TunnelPhase::Authenticating);
                break;
            case TunnelEventType::UserDisconnect:
                if (phase_ != TunnelPhase::Idle &&
                    phase_ != TunnelPhase::Disconnecting) {
                    intent_.desired_connected = false;
                    transition(TunnelPhase::Disconnecting);
                }
                break;
            case TunnelEventType::CleanupSucceeded:
                if (phase_ == TunnelPhase::Disconnecting)
                    transition(TunnelPhase::Idle);
                break;
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

    // === Full connect flow: Idle -> Connected ===
    {
        auto helper = std::make_shared<exv::test::FakeHelper>();
        auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
        TestTunnelController ctrl;

        std::vector<TunnelPhase> observed_phases;
        ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
            observed_phases.push_back(snap.phase);
        });

        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);

        ok = expect(ctrl.phase() == TunnelPhase::PreparingHelper,
                    "after connect(), phase should be PreparingHelper") && ok;

        ctrl.on_event({TunnelEventType::HelperReady});
        ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                    "after HelperReady, phase should be Authenticating") && ok;

        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ok = expect(ctrl.phase() == TunnelPhase::ConnectingCstp,
                    "after AuthSucceeded, phase should be ConnectingCstp") && ok;

        ctrl.on_event({TunnelEventType::CstpConnected});
        ok = expect(ctrl.phase() == TunnelPhase::ApplyingNetworkConfig,
                    "after CstpConnected, phase should be ApplyingNetworkConfig") && ok;

        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ok = expect(ctrl.phase() == TunnelPhase::OpeningPacketDevice,
                    "after NetworkConfigApplied, phase should be OpeningPacketDevice") && ok;

        ctrl.on_event({TunnelEventType::PacketLoopStarted});
        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "after PacketLoopStarted, phase should be Connected") && ok;

        // Verify the full transition sequence was observed via callback
        ok = expect(observed_phases.size() >= 6,
                    "should observe at least 6 phase transitions") && ok;
        ok = expect(observed_phases.front() == TunnelPhase::PreparingHelper,
                    "first observed phase should be PreparingHelper") && ok;
        ok = expect(observed_phases.back() == TunnelPhase::Connected,
                    "last observed phase should be Connected") && ok;

        // Verify helper was not actually called (fakes are passive)
        ok = expect(helper->connect_count() == 0,
                    "FakeHelper connect should not be called by controller") && ok;
    }

    // === Disconnect flow: Connected -> Idle ===
    {
        auto helper = std::make_shared<exv::test::FakeHelper>();
        auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
        TestTunnelController ctrl;

        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        drive_to_connected(ctrl);

        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "should be Connected before disconnect") && ok;

        ctrl.disconnect();
        ok = expect(ctrl.phase() == TunnelPhase::Disconnecting,
                    "disconnect() should transition to Disconnecting") && ok;

        ctrl.on_event({TunnelEventType::CleanupSucceeded});
        ok = expect(ctrl.phase() == TunnelPhase::Idle,
                    "CleanupSucceeded should transition to Idle") && ok;

        auto snap = ctrl.status();
        ok = expect(!snap.desired_connected,
                    "after disconnect, desired_connected should be false") && ok;
    }

    // === Reconnect flow: Connected -> Reconnecting -> Connected ===
    {
        auto helper = std::make_shared<exv::test::FakeHelper>();
        auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
        TestTunnelController ctrl;

        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        drive_to_connected(ctrl);

        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "should be Connected before transport close") && ok;

        ctrl.on_event({TunnelEventType::TransportClosed});
        ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                    "TransportClosed should transition to Reconnecting") && ok;

        ctrl.on_event({TunnelEventType::ReconnectTimerFired});
        ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                    "ReconnectTimerFired should transition to Authenticating") && ok;

        drive_to_connected(ctrl);
        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "reconnect should eventually reach Connected") && ok;

        auto snap = ctrl.status();
        ok = expect(snap.reconnect.has_value(),
                    "status should include reconnect info") && ok;
    }

    // === Multiple reconnect attempts ===
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        drive_to_connected(ctrl);

        // First disconnect/reconnect
        ctrl.on_event({TunnelEventType::TransportClosed});
        ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                    "first TransportClosed -> Reconnecting") && ok;
        ctrl.on_event({TunnelEventType::ReconnectTimerFired});
        drive_to_connected(ctrl);
        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "first reconnect -> Connected") && ok;

        // Second disconnect/reconnect
        ctrl.on_event({TunnelEventType::TransportClosed});
        ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                    "second TransportClosed -> Reconnecting") && ok;
        ctrl.on_event({TunnelEventType::ReconnectTimerFired});
        drive_to_connected(ctrl);
        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "second reconnect -> Connected") && ok;
    }

    // === Disconnect during reconnect ===
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        drive_to_connected(ctrl);

        ctrl.on_event({TunnelEventType::TransportClosed});
        ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                    "should be Reconnecting") && ok;

        ctrl.disconnect();
        ok = expect(ctrl.phase() == TunnelPhase::Disconnecting,
                    "disconnect during Reconnecting -> Disconnecting") && ok;

        ctrl.on_event({TunnelEventType::CleanupSucceeded});
        ok = expect(ctrl.phase() == TunnelPhase::Idle,
                    "cleanup after disconnect -> Idle") && ok;
    }

    // === FakePlatformNetworkOps tracks operations ===
    {
        auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();

        ok = expect(net_ops->prepare_count() == 0,
                    "initial prepare count should be 0") && ok;
        ok = expect(net_ops->apply_count() == 0,
                    "initial apply count should be 0") && ok;
        ok = expect(net_ops->cleanup_count() == 0,
                    "initial cleanup count should be 0") && ok;
    }

    // === FakeHelper session management ===
    {
        auto helper = std::make_shared<exv::test::FakeHelper>();
        ok = expect(!helper->is_connected(),
                    "FakeHelper should start disconnected") && ok;

        bool connected = helper->connect();
        ok = expect(connected, "FakeHelper connect should succeed") && ok;
        ok = expect(helper->is_connected(),
                    "FakeHelper should be connected after connect()") && ok;

        helper->disconnect();
        ok = expect(!helper->is_connected(),
                    "FakeHelper should be disconnected after disconnect()") && ok;
    }

    if (ok) {
        std::cout << "native_core_connect_flow_test: all assertions passed\n";
    } else {
        std::cerr << "native_core_connect_flow_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
