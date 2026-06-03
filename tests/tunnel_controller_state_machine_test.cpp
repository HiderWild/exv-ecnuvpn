// TunnelController state machine test.
//
// This test defines a standalone state machine that mirrors the expected
// TunnelController behavior.  It verifies all required state transitions
// using the core type definitions (TunnelPhase, TunnelEvent, etc.).
//
// When a production TunnelController implementation lands (with proper
// type aliases for HelperClient/PlatformNetworkOps), this test can be
// updated to use it directly.

#include "core/tunnel_intent.hpp"
#include "core/tunnel_state.hpp"
#include "core/tunnel_events.hpp"
#include "core/reconnect_policy.hpp"

#include <iostream>
#include <string>
#include <functional>
#include <memory>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

// Standalone state machine that mirrors TunnelController behavior.
// Uses the core types directly without depending on tunnel_controller.hpp.
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
            case TunnelEventType::PacketDeviceFailed:
                transition(TunnelPhase::Failed);
                break;
            case TunnelEventType::HelperLost:
                if (phase_ != TunnelPhase::Idle)
                    transition(TunnelPhase::Failed);
                break;
            default:
                break;
        }
    }

    void set_status_callback(StatusCallback cb) {
        status_cb_ = std::move(cb);
    }

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

} // namespace

int main() {
    bool ok = true;

    using exv::core::TunnelPhase;
    using exv::core::TunnelEvent;
    using exv::core::TunnelEventType;
    using exv::core::UserIntent;
    using exv::core::DisconnectReason;

    // --- Idle -> connect() -> PreparingHelper ---
    {
        TestTunnelController ctrl;
        ok = expect(ctrl.phase() == TunnelPhase::Idle,
                    "initial phase should be Idle") && ok;

        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ok = expect(ctrl.phase() == TunnelPhase::PreparingHelper,
                    "connect() should transition to PreparingHelper") && ok;
    }

    // --- PreparingHelper -> HelperReady -> Authenticating ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);

        ctrl.on_event({TunnelEventType::HelperReady});
        ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                    "HelperReady should transition to Authenticating") && ok;
    }

    // --- Authenticating -> AuthSucceeded -> ConnectingCstp ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});

        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ok = expect(ctrl.phase() == TunnelPhase::ConnectingCstp,
                    "AuthSucceeded should transition to ConnectingCstp") && ok;
    }

    // --- ConnectingCstp -> CstpConnected -> ApplyingNetworkConfig ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});

        ctrl.on_event({TunnelEventType::CstpConnected});
        ok = expect(ctrl.phase() == TunnelPhase::ApplyingNetworkConfig,
                    "CstpConnected should transition to ApplyingNetworkConfig") && ok;
    }

    // --- ApplyingNetworkConfig -> NetworkConfigApplied -> OpeningPacketDevice ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});

        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ok = expect(ctrl.phase() == TunnelPhase::OpeningPacketDevice,
                    "NetworkConfigApplied should transition to OpeningPacketDevice") && ok;
    }

    // --- OpeningPacketDevice -> PacketLoopStarted -> Connected ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ctrl.on_event({TunnelEventType::NetworkConfigApplied});

        ctrl.on_event({TunnelEventType::PacketLoopStarted});
        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "PacketLoopStarted should transition to Connected") && ok;
    }

    // --- Connected -> TransportClosed + auto_reconnect -> Reconnecting ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ctrl.on_event({TunnelEventType::PacketLoopStarted});

        ctrl.on_event({TunnelEventType::TransportClosed});
        ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                    "TransportClosed + auto_reconnect should transition to Reconnecting") && ok;
    }

    // --- Reconnecting -> ReconnectTimerFired -> Authenticating ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ctrl.on_event({TunnelEventType::PacketLoopStarted});
        ctrl.on_event({TunnelEventType::TransportClosed});

        ctrl.on_event({TunnelEventType::ReconnectTimerFired});
        ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                    "ReconnectTimerFired should transition to Authenticating") && ok;
    }

    // --- Any state -> UserDisconnect -> Disconnecting ---
    {
        // From Connected
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ctrl.on_event({TunnelEventType::PacketLoopStarted});

        ctrl.on_event({TunnelEventType::UserDisconnect});
        ok = expect(ctrl.phase() == TunnelPhase::Disconnecting,
                    "UserDisconnect from Connected should go to Disconnecting") && ok;
    }

    {
        // From Authenticating
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});

        ctrl.on_event({TunnelEventType::UserDisconnect});
        ok = expect(ctrl.phase() == TunnelPhase::Disconnecting,
                    "UserDisconnect from Authenticating should go to Disconnecting") && ok;
    }

    {
        // From PreparingHelper
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);

        ctrl.on_event({TunnelEventType::UserDisconnect});
        ok = expect(ctrl.phase() == TunnelPhase::Disconnecting,
                    "UserDisconnect from PreparingHelper should go to Disconnecting") && ok;
    }

    // --- Disconnecting -> CleanupSucceeded -> Idle ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ctrl.on_event({TunnelEventType::PacketLoopStarted});
        ctrl.on_event({TunnelEventType::UserDisconnect});

        ctrl.on_event({TunnelEventType::CleanupSucceeded});
        ok = expect(ctrl.phase() == TunnelPhase::Idle,
                    "CleanupSucceeded should transition to Idle") && ok;
    }

    // --- Connected -> AuthFailed -> Failed (non-recoverable) ---
    {
        TestTunnelController ctrl;
        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ctrl.on_event({TunnelEventType::PacketLoopStarted});

        ctrl.on_event({TunnelEventType::AuthFailed});
        ok = expect(ctrl.phase() == TunnelPhase::Failed,
                    "AuthFailed should transition to Failed") && ok;
    }

    // --- Disconnecting from Idle is a no-op ---
    {
        TestTunnelController ctrl;
        ctrl.on_event({TunnelEventType::UserDisconnect});
        ok = expect(ctrl.phase() == TunnelPhase::Idle,
                    "UserDisconnect from Idle should stay Idle") && ok;
    }

    // --- HelperReady in wrong state is ignored ---
    {
        TestTunnelController ctrl;
        ctrl.on_event({TunnelEventType::HelperReady});
        ok = expect(ctrl.phase() == TunnelPhase::Idle,
                    "HelperReady in Idle should be ignored") && ok;
    }

    // --- TransportClosed without auto_reconnect -> Failed ---
    {
        TestTunnelController ctrl;
        ctrl.set_auto_reconnect(false);
        UserIntent intent;
        intent.desired_connected = true;
        intent.auto_reconnect = false;
        ctrl.connect(intent);
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ctrl.on_event({TunnelEventType::PacketLoopStarted});

        ctrl.on_event({TunnelEventType::TransportClosed});
        ok = expect(ctrl.phase() == TunnelPhase::Failed,
                    "TransportClosed without auto_reconnect should go to Failed") && ok;
    }

    // --- Status callback is invoked on transitions ---
    {
        TestTunnelController ctrl;
        int callback_count = 0;
        TunnelPhase last_phase = TunnelPhase::Idle;
        ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
            callback_count++;
            last_phase = snap.phase;
        });

        UserIntent intent;
        intent.desired_connected = true;
        ctrl.connect(intent);
        ok = expect(callback_count >= 1,
                    "status callback should be invoked on connect") && ok;
        ok = expect(last_phase == TunnelPhase::PreparingHelper,
                    "callback should report PreparingHelper") && ok;
    }

    // --- Status snapshot reflects current state ---
    {
        TestTunnelController ctrl;
        auto snap = ctrl.status();
        ok = expect(snap.phase == TunnelPhase::Idle,
                    "initial status should be Idle") && ok;
        ok = expect(!snap.desired_connected,
                    "initial desired_connected should be false") && ok;
    }

    if (ok) {
        std::cout << "tunnel_controller_state_machine_test: all assertions passed\n";
    } else {
        std::cerr << "tunnel_controller_state_machine_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
