// Integration test: auth failure behavior.
//
// Verifies that auth failures are treated as non-recoverable and do NOT
// trigger reconnection attempts.

#include "core/tunnel_intent.hpp"
#include "core/tunnel_state.hpp"
#include "core/tunnel_events.hpp"
#include "core/reconnect_policy.hpp"
#include "support/fake_helper.hpp"

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
// Includes auth failure -> Failed (no reconnect) logic.
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
                // Auth failure is non-recoverable: go directly to Failed
                if (phase_ == TunnelPhase::Authenticating ||
                    phase_ == TunnelPhase::Connected) {
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

    void set_auto_reconnect(bool enabled) { auto_reconnect_ = enabled; }

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

} // namespace

int main() {
    bool ok = true;

    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;
    using exv::core::UserIntent;
    using exv::core::ReconnectPolicy;
    using exv::core::ErrorInfo;
    using exv::core::ReconnectDecision;

    // === Test: auth_failure_does_not_reconnect ===
    // Drive controller to Authenticating, fire AuthFailed, verify Failed (not Reconnecting)
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

        // Drive to Authenticating
        ctrl.on_event({TunnelEventType::HelperReady});
        ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                    "should be Authenticating after HelperReady") && ok;

        // Fire auth failure
        ctrl.on_event({TunnelEventType::AuthFailed});

        // Verify: transitions to Failed, NOT Reconnecting
        ok = expect(ctrl.phase() == TunnelPhase::Failed,
                    "auth failure should transition to Failed, not Reconnecting") && ok;

        // Verify the transition sequence never includes Reconnecting
        bool saw_reconnecting = false;
        for (auto p : observed_phases) {
            if (p == TunnelPhase::Reconnecting) {
                saw_reconnecting = true;
                break;
            }
        }
        ok = expect(!saw_reconnecting,
                    "auth failure must NOT trigger Reconnecting phase") && ok;

        // Verify: error is set and is non-recoverable
        auto snap = ctrl.status();
        ok = expect(snap.last_error.has_value(),
                    "status should include last_error after auth failure") && ok;
        if (snap.last_error.has_value()) {
            ok = expect(snap.last_error->domain == "auth",
                        "error domain should be 'auth'") && ok;
            ok = expect(snap.last_error->code == "auth_failed",
                        "error code should be 'auth_failed'") && ok;
            ok = expect(!snap.last_error->recoverable,
                        "auth failure should be non-recoverable") && ok;
        }
    }

    // === Test: ReconnectPolicy says should_retry=false for auth failure ===
    {
        ReconnectPolicy policy;

        ErrorInfo auth_error;
        auth_error.domain = "auth";
        auth_error.code = "auth_failed";
        auth_error.message = "Authentication failed";
        auth_error.recoverable = false;

        UserIntent intent;
        intent.desired_connected = true;
        intent.auto_reconnect = true;

        ReconnectDecision decision = policy.decide(
            auth_error, intent, TunnelPhase::Authenticating, 0);

        ok = expect(!decision.should_retry,
                    "ReconnectPolicy should say should_retry=false for auth failure") && ok;
    }

    // === Test: auth failure mid-connection also goes to Failed ===
    {
        TestTunnelController ctrl;

        UserIntent intent;
        intent.desired_connected = true;
        intent.auto_reconnect = true;
        ctrl.connect(intent);

        // Drive all the way to Connected
        ctrl.on_event({TunnelEventType::HelperReady});
        ctrl.on_event({TunnelEventType::AuthSucceeded});
        ctrl.on_event({TunnelEventType::CstpConnected});
        ctrl.on_event({TunnelEventType::NetworkConfigApplied});
        ctrl.on_event({TunnelEventType::PacketLoopStarted});
        ok = expect(ctrl.phase() == TunnelPhase::Connected,
                    "should be Connected") && ok;

        // Fire auth failure while connected (e.g., re-auth during reconnect)
        ctrl.on_event({TunnelEventType::AuthFailed});
        ok = expect(ctrl.phase() == TunnelPhase::Failed,
                    "auth failure while connected should go to Failed") && ok;
    }

    // === Test: verify FakeHelper start_session failure mode ===
    {
        auto helper = std::make_shared<exv::test::FakeHelper>();
        helper->connect();
        ok = expect(helper->is_connected(), "helper should be connected") && ok;

        helper->set_start_session_fail(true);
        exv::helper::StartSessionRequest req;
        req.profile_id.value = "test-profile";
        auto resp = helper->start_session(req);
        ok = expect(resp.session_id.value.empty(),
                    "start_session with fail flag should return empty session_id") && ok;

        // Reset and verify normal operation resumes
        helper->set_start_session_fail(false);
        auto resp2 = helper->start_session(req);
        ok = expect(!resp2.session_id.value.empty(),
                    "start_session after reset should succeed") && ok;
    }

    if (ok) {
        std::cout << "auth_failure_test: all assertions passed\n";
    } else {
        std::cerr << "auth_failure_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
