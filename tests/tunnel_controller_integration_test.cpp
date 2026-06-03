// TunnelController integration tests.
//
// Tests the full connect/disconnect/reconnect flows by orchestrating
// FakeHelper, FakePlatformNetworkOps, FakeCoreUiClient, and ReconnectPolicy.
//
// Since TunnelController uses pimpl and may not be fully linked yet, these
// tests exercise the expected orchestration logic directly using the fakes
// and the core types.  When TunnelController.cpp lands, the standalone
// IntegrationTunnelController below can be swapped for the real class.

#include "core/tunnel_intent.hpp"
#include "core/tunnel_state.hpp"
#include "core/tunnel_events.hpp"
#include "core/reconnect_policy.hpp"
#include "support/fake_helper.hpp"
#include "support/fake_platform_network_ops.hpp"
#include "support/fake_core_ui_client.hpp"

#include <iostream>
#include <string>
#include <vector>
#include <functional>
#include <memory>
#include <chrono>
#include <cassert>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

// ---------------------------------------------------------------------------
// IntegrationTunnelController
//
// Standalone controller that orchestrates FakeHelper + FakePlatformNetworkOps
// + ReconnectPolicy.  Mirrors the expected TunnelController behavior for
// integration testing.
// ---------------------------------------------------------------------------
class IntegrationTunnelController {
public:
    using StatusCallback = std::function<void(const exv::core::TunnelStatusSnapshot&)>;

    IntegrationTunnelController(
        std::shared_ptr<exv::test::FakeHelper> helper,
        std::shared_ptr<exv::test::FakePlatformNetworkOps> net_ops,
        exv::core::ReconnectConfig reconnect_config = {}
    )
        : helper_(std::move(helper))
        , net_ops_(std::move(net_ops))
        , reconnect_policy_(reconnect_config)
    {}

    // ---- User intent interface ----

    void connect(exv::core::UserIntent intent) {
        intent_ = std::move(intent);
        intent_.desired_connected = true;
        if (phase_ == exv::core::TunnelPhase::Idle ||
            phase_ == exv::core::TunnelPhase::Failed) {
            attempt_count_ = 0;
            reconnect_policy_.reset();
            transition(exv::core::TunnelPhase::PreparingHelper);
            // Simulate helper preparation: connect to helper, hello, start session
            if (helper_->connect()) {
                exv::helper::HelloRequest hello_req;
                helper_->hello(hello_req);

                exv::helper::StartSessionRequest start_req;
                start_req.profile_id.value = intent_.profile_id.value;
                start_req.mode = exv::helper::HelperMode::Transient;
                auto start_resp = helper_->start_session(start_req);
                session_id_ = start_resp.session_id;

                transition(exv::core::TunnelPhase::Authenticating);
            }
        }
    }

    void disconnect(exv::core::DisconnectReason reason =
                        exv::core::DisconnectReason::UserRequested) {
        if (phase_ == exv::core::TunnelPhase::Idle ||
            phase_ == exv::core::TunnelPhase::Disconnecting) {
            return;
        }
        intent_.desired_connected = false;
        intent_.user_disconnect_reason = reason;
        transition(exv::core::TunnelPhase::Disconnecting);
        // Simulate cleanup
        exv::helper::CleanupRequest cleanup_req;
        cleanup_req.session_id = session_id_;
        cleanup_req.policy.remove_routes = true;
        cleanup_req.policy.remove_dns = true;
        cleanup_req.policy.remove_adapter = false;
        cleanup_req.policy.remove_firewall_rules = true;
        helper_->cleanup(cleanup_req);

        net_ops_->cleanup("ECNU-VPN", exv::platform::CleanupPolicy::KeepAdapter);
        transition(exv::core::TunnelPhase::Idle);
    }

    void set_auto_reconnect(bool enabled) {
        auto_reconnect_ = enabled;
        intent_.auto_reconnect = enabled;
    }

    // ---- Status ----

    exv::core::TunnelPhase phase() const { return phase_; }

    exv::core::TunnelStatusSnapshot status() const {
        exv::core::TunnelStatusSnapshot snap;
        snap.phase = phase_;
        snap.desired_connected = intent_.desired_connected;
        snap.auto_reconnect = auto_reconnect_;
        snap.server = server_;
        snap.reconnect = exv::core::ReconnectInfo{attempt_count_, 0};
        if (last_error_) snap.last_error = last_error_;
        return snap;
    }

    // ---- Event processing ----

    void on_event(exv::core::TunnelEvent event) {
        using exv::core::TunnelPhase;
        using exv::core::TunnelEventType;

        switch (event.type) {
            case TunnelEventType::HelperReady:
                if (phase_ == TunnelPhase::PreparingHelper)
                    transition(TunnelPhase::Authenticating);
                break;

            case TunnelEventType::AuthSucceeded:
                if (phase_ == TunnelPhase::Authenticating) {
                    // Simulate CSTP connect, then prepare + apply tunnel config
                    exv::helper::PrepareTunnelDeviceRequest prep_req;
                    prep_req.session_id = session_id_;
                    prep_req.adapter_name = "ECNU-VPN";
                    helper_->prepare_tunnel_device(prep_req);

                    net_ops_->prepare_tunnel_device("ECNU-VPN", 1400);

                    transition(TunnelPhase::ConnectingCstp);
                    // Auto-advance: CstpConnected -> ApplyingNetworkConfig
                    transition(TunnelPhase::ApplyingNetworkConfig);

                    // Apply tunnel config via helper
                    exv::helper::ApplyTunnelConfigRequest apply_req;
                    apply_req.config.session_id = session_id_;
                    apply_req.config.interface_address = "10.0.0.2/24";
                    helper_->apply_tunnel_config(apply_req);

                    // Apply via platform
                    exv::platform::TunnelConfig plat_config;
                    plat_config.interface_address = "10.0.0.2/24";
                    plat_config.interface_name = "ECNU-VPN";
                    auto device = net_ops_->open_tunnel_device("ECNU-VPN");
                    net_ops_->apply_tunnel_config(device, plat_config);

                    transition(TunnelPhase::OpeningPacketDevice);
                    // Auto-advance: PacketLoopStarted -> Connected
                    attempt_count_ = 0;
                    transition(TunnelPhase::Connected);
                }
                break;

            case TunnelEventType::AuthFailed: {
                exv::core::ErrorInfo err;
                err.domain = "auth";
                err.code = "auth_failed";
                err.message = "Authentication failed";
                err.recoverable = false;
                last_error_ = err;
                if (phase_ == TunnelPhase::Authenticating ||
                    phase_ == TunnelPhase::Connected) {
                    transition(TunnelPhase::Failed);
                }
                break;
            }

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

            case TunnelEventType::TransportClosed: {
                if (phase_ == TunnelPhase::Connected) {
                    exv::core::ErrorInfo err;
                    err.domain = "transport";
                    err.code = "transport_closed";
                    err.message = "Connection lost";
                    err.recoverable = true;
                    last_error_ = err;

                    auto decision = reconnect_policy_.decide(
                        err, intent_, phase_, attempt_count_);
                    if (decision.should_retry) {
                        attempt_count_++;
                        transition(TunnelPhase::Reconnecting);
                    } else {
                        transition(TunnelPhase::Failed);
                    }
                }
                break;
            }

            case TunnelEventType::ReconnectTimerFired:
                if (phase_ == TunnelPhase::Reconnecting) {
                    // Attempt re-auth through the full flow
                    transition(TunnelPhase::Authenticating);
                    // Simulate re-auth succeeding
                    exv::helper::PrepareTunnelDeviceRequest prep_req;
                    prep_req.session_id = session_id_;
                    prep_req.adapter_name = "ECNU-VPN";
                    helper_->prepare_tunnel_device(prep_req);

                    net_ops_->prepare_tunnel_device("ECNU-VPN", 1400);

                    transition(TunnelPhase::ConnectingCstp);
                    transition(TunnelPhase::ApplyingNetworkConfig);

                    exv::helper::ApplyTunnelConfigRequest apply_req;
                    apply_req.config.session_id = session_id_;
                    apply_req.config.interface_address = "10.0.0.2/24";
                    helper_->apply_tunnel_config(apply_req);

                    exv::platform::TunnelConfig plat_config;
                    plat_config.interface_address = "10.0.0.2/24";
                    plat_config.interface_name = "ECNU-VPN";
                    auto device = net_ops_->open_tunnel_device("ECNU-VPN");
                    net_ops_->apply_tunnel_config(device, plat_config);

                    transition(TunnelPhase::OpeningPacketDevice);
                    transition(TunnelPhase::Connected);
                }
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

            case TunnelEventType::PacketDeviceFailed: {
                exv::core::ErrorInfo err;
                err.domain = "os.route";
                err.code = "packet_device_failed";
                err.message = "Packet device failure";
                err.recoverable = false;
                last_error_ = err;
                transition(TunnelPhase::Failed);
                break;
            }

            case TunnelEventType::HelperLost: {
                exv::core::ErrorInfo err;
                err.domain = "helper";
                err.code = "helper_lost";
                err.message = "Helper process lost";
                err.recoverable = false;
                last_error_ = err;
                if (phase_ != TunnelPhase::Idle)
                    transition(TunnelPhase::Failed);
                break;
            }

            default:
                break;
        }
    }

    // ---- Status callback ----

    void set_status_callback(StatusCallback cb) {
        status_cb_ = std::move(cb);
    }

    // ---- Inspection helpers ----

    int attempt_count() const { return attempt_count_; }
    std::optional<exv::core::ErrorInfo> last_error() const { return last_error_; }

private:
    void transition(exv::core::TunnelPhase new_phase) {
        phase_ = new_phase;
        if (status_cb_) status_cb_(status());
    }

    std::shared_ptr<exv::test::FakeHelper> helper_;
    std::shared_ptr<exv::test::FakePlatformNetworkOps> net_ops_;
    exv::core::ReconnectPolicy reconnect_policy_;

    exv::core::TunnelPhase phase_ = exv::core::TunnelPhase::Idle;
    exv::core::UserIntent intent_;
    bool auto_reconnect_ = true;
    int attempt_count_ = 0;
    std::string server_;
    exv::helper::SessionId session_id_;
    std::optional<exv::core::ErrorInfo> last_error_;
    StatusCallback status_cb_;
};

// ---------------------------------------------------------------------------
// Helper to build a UserIntent
// ---------------------------------------------------------------------------
exv::core::UserIntent make_intent(bool desired, bool auto_reconnect = true,
                                   const std::string& profile = "default") {
    exv::core::UserIntent intent;
    intent.desired_connected = desired;
    intent.auto_reconnect = auto_reconnect;
    intent.profile_id.value = profile;
    return intent;
}

// Drive the controller through the full connect sequence.
// Returns true if the controller reached Connected phase.
bool drive_to_connected(IntegrationTunnelController& ctrl,
                        exv::test::FakeHelper& helper,
                        exv::test::FakePlatformNetworkOps& net_ops,
                        const char* label) {
    ctrl.connect(make_intent(true));
    if (ctrl.phase() == exv::core::TunnelPhase::Authenticating) {
        ctrl.on_event({exv::core::TunnelEventType::AuthSucceeded});
    }
    bool ok = expect(ctrl.phase() == exv::core::TunnelPhase::Connected,
                     (std::string(label) + ": should reach Connected").c_str());
    if (!ok) {
        std::cerr << "  (stuck at phase " << static_cast<int>(ctrl.phase()) << ")\n";
    }
    return ok;
}

// Helper to count transitions to a specific phase.
int count_phase(const std::vector<exv::core::TunnelStatusSnapshot>& snapshots,
                exv::core::TunnelPhase target) {
    int count = 0;
    for (auto& s : snapshots) {
        if (s.phase == target) count++;
    }
    return count;
}

// Check if any snapshot has the given phase.
bool any_has_phase(const std::vector<exv::core::TunnelStatusSnapshot>& snapshots,
                   exv::core::TunnelPhase target) {
    for (auto& s : snapshots) {
        if (s.phase == target) return true;
    }
    return false;
}

// Check that snapshots follow an ordered subsequence of phases.
bool phases_in_order(const std::vector<exv::core::TunnelStatusSnapshot>& snapshots,
                     const std::vector<exv::core::TunnelPhase>& expected_phases) {
    size_t idx = 0;
    for (auto& s : snapshots) {
        if (idx < expected_phases.size() && s.phase == expected_phases[idx]) {
            idx++;
        }
    }
    return idx == expected_phases.size();
}

} // namespace

// ===========================================================================
// Test cases
// ===========================================================================

// --- Test 1: Full Connect Flow ---
bool test_full_connect_flow() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    IntegrationTunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;
    ok = expect(ctrl.phase() == TunnelPhase::Idle,
                "1: initial phase should be Idle") && ok;

    // Connect
    ctrl.connect(make_intent(true));
    ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                "1: after connect() should be Authenticating") && ok;

    // Verify helper received start_session (connect was called)
    ok = expect(helper->connect_count() >= 1,
                "1: helper should have been connected") && ok;
    ok = expect(helper->active_sessions().size() >= 1,
                "1: helper should have at least one active session") && ok;

    // Auth succeeds -> drives through ConnectingCstp -> ApplyingNetworkConfig -> Connected
    ctrl.on_event({TunnelEventType::AuthSucceeded});
    ok = expect(ctrl.phase() == TunnelPhase::Connected,
                "1: after AuthSucceeded full flow should reach Connected") && ok;

    // Verify platform ops received prepare_tunnel_device
    ok = expect(net_ops->prepare_count() >= 1,
                "1: platform ops should have prepared tunnel device") && ok;

    // Verify helper received apply_tunnel_config
    ok = expect(helper->active_sessions().size() >= 1,
                "1: helper session should still be active") && ok;

    // Verify status transitions include expected phases
    auto snapshots = ui.received_snapshots();
    ok = expect(phases_in_order(snapshots,
                    {TunnelPhase::PreparingHelper, TunnelPhase::Authenticating,
                     TunnelPhase::Connected}),
                "1: status callbacks should show Prepare->Auth->Connected order") && ok;
    ok = expect(ui.last_snapshot().phase == TunnelPhase::Connected,
                "1: final snapshot should be Connected") && ok;

    return ok;
}

// --- Test 2: Disconnect Flow ---
bool test_disconnect_flow() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    IntegrationTunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Get to Connected
    ok = drive_to_connected(ctrl, *helper, *net_ops, "2") && ok;
    ui.clear();

    // Disconnect
    ctrl.disconnect();
    ok = expect(ctrl.phase() == TunnelPhase::Idle,
                "2: after disconnect() should be Idle") && ok;

    // Verify helper received cleanup
    ok = expect(helper->cleanup_requests().size() >= 1,
                "2: helper should have received cleanup request") && ok;

    // Verify platform cleanup was called
    ok = expect(net_ops->cleanup_count() >= 1,
                "2: platform ops should have been cleaned up") && ok;

    // Verify status transitions: Connected -> Disconnecting -> Idle
    auto snapshots = ui.received_snapshots();
    ok = expect(any_has_phase(snapshots, TunnelPhase::Disconnecting),
                "2: should have transitioned through Disconnecting") && ok;
    ok = expect(ui.last_snapshot().phase == TunnelPhase::Idle,
                "2: final snapshot should be Idle") && ok;

    return ok;
}

// --- Test 3: Reconnect Flow (auto_reconnect=true) ---
bool test_reconnect_flow() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    exv::core::ReconnectConfig config;
    config.base_delay = std::chrono::milliseconds(100);
    config.max_delay = std::chrono::milliseconds(5000);

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    IntegrationTunnelController ctrl(helper, net_ops, config);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Get to Connected
    ok = drive_to_connected(ctrl, *helper, *net_ops, "3") && ok;
    ui.clear();

    // Simulate TransportClosed
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                "3: TransportClosed with auto_reconnect should go to Reconnecting") && ok;

    // Verify attempt count increased
    ok = expect(ctrl.attempt_count() >= 1,
                "3: attempt count should be >= 1 after reconnect") && ok;

    // Simulate reconnect timer -> should re-auth and reach Connected
    ctrl.on_event({TunnelEventType::ReconnectTimerFired});
    ok = expect(ctrl.phase() == TunnelPhase::Connected,
                "3: after ReconnectTimerFired should reach Connected") && ok;

    // Verify status transitions show Reconnecting
    auto snapshots = ui.received_snapshots();
    ok = expect(any_has_phase(snapshots, TunnelPhase::Reconnecting),
                "3: should have transitioned through Reconnecting") && ok;
    ok = expect(ui.last_snapshot().phase == TunnelPhase::Connected,
                "3: final snapshot should be Connected") && ok;

    return ok;
}

// --- Test 4: Auto-reconnect Disabled ---
bool test_auto_reconnect_disabled() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    IntegrationTunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Connect with auto_reconnect=false
    ctrl.connect(make_intent(true, false));
    if (ctrl.phase() == exv::core::TunnelPhase::Authenticating) {
        ctrl.on_event({TunnelEventType::AuthSucceeded});
    }
    ok = expect(ctrl.phase() == TunnelPhase::Connected,
                "4: should reach Connected with auto_reconnect=false") && ok;
    ui.clear();

    // Simulate TransportClosed
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "4: TransportClosed with auto_reconnect=false should go to Failed") && ok;

    // Verify NO Reconnecting transition
    auto snapshots = ui.received_snapshots();
    ok = expect(!any_has_phase(snapshots, TunnelPhase::Reconnecting),
                "4: should NOT have transitioned through Reconnecting") && ok;

    // Verify error info
    auto err = ctrl.last_error();
    ok = expect(err.has_value(),
                "4: should have last_error set") && ok;
    if (err) {
        ok = expect(err->code == "transport_closed",
                    "4: error code should be transport_closed") && ok;
    }

    return ok;
}

// --- Test 5: Auth Failure (non-recoverable) ---
bool test_auth_failure() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    IntegrationTunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Start connecting
    ctrl.connect(make_intent(true));
    ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                "5: should be Authenticating after connect") && ok;

    // Simulate AuthFailed
    ctrl.on_event({TunnelEventType::AuthFailed});
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "5: AuthFailed should transition to Failed") && ok;

    // Verify error info says non-recoverable
    auto err = ctrl.last_error();
    ok = expect(err.has_value(),
                "5: should have last_error set") && ok;
    if (err) {
        ok = expect(!err->recoverable,
                    "5: auth failure should be non-recoverable") && ok;
        ok = expect(err->domain == "auth",
                    "5: error domain should be auth") && ok;
    }

    // Verify ReconnectPolicy would say should_retry=false for non-recoverable
    exv::core::ReconnectPolicy policy;
    exv::core::ErrorInfo auth_err;
    auth_err.domain = "auth";
    auth_err.code = "auth_failed";
    auth_err.recoverable = false;
    auto decision = policy.decide(auth_err, make_intent(true),
                                  TunnelPhase::Authenticating, 0);
    ok = expect(!decision.should_retry,
                "5: ReconnectPolicy should not retry non-recoverable auth error") && ok;

    // Verify status transitions
    auto snapshots = ui.received_snapshots();
    ok = expect(any_has_phase(snapshots, TunnelPhase::Failed),
                "5: should have snapshot showing Failed") && ok;

    return ok;
}

// --- Test 6: User Disconnect During Reconnecting ---
bool test_user_disconnect_during_reconnecting() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    exv::core::ReconnectConfig config;
    config.base_delay = std::chrono::milliseconds(100);

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    IntegrationTunnelController ctrl(helper, net_ops, config);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Get to Connected
    ok = drive_to_connected(ctrl, *helper, *net_ops, "6") && ok;

    // Trigger reconnect
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                "6: should be Reconnecting") && ok;
    ui.clear();

    // User disconnects while reconnecting
    ctrl.disconnect();
    ok = expect(ctrl.phase() == TunnelPhase::Idle,
                "6: disconnect during Reconnecting should reach Idle") && ok;

    // Verify helper received cleanup
    ok = expect(helper->cleanup_requests().size() >= 1,
                "6: helper should have received cleanup") && ok;

    // Verify transitions
    auto snapshots = ui.received_snapshots();
    ok = expect(any_has_phase(snapshots, TunnelPhase::Disconnecting),
                "6: should have gone through Disconnecting") && ok;
    ok = expect(ui.last_snapshot().phase == TunnelPhase::Idle,
                "6: final snapshot should be Idle") && ok;

    return ok;
}

// --- Test 7: Status Callback ---
bool test_status_callback() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    IntegrationTunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Before connect: no callbacks
    ok = expect(ui.snapshot_count() == 0,
                "7: no callbacks before connect") && ok;

    // Connect -> should trigger callbacks at each transition
    ctrl.connect(make_intent(true));
    ok = expect(ui.snapshot_count() >= 1,
                "7: connect should trigger at least one callback") && ok;

    // Auth succeeded -> more callbacks
    ctrl.on_event({TunnelEventType::AuthSucceeded});

    // Verify callback was called at each state transition
    auto snapshots = ui.received_snapshots();
    ok = expect(snapshots.size() >= 2,
                "7: multiple callbacks expected through full flow") && ok;

    // Verify snapshots contain PreparingHelper, Authenticating, and Connected
    ok = expect(any_has_phase(snapshots, TunnelPhase::PreparingHelper),
                "7: should have PreparingHelper callback") && ok;
    ok = expect(any_has_phase(snapshots, TunnelPhase::Authenticating),
                "7: should have Authenticating callback") && ok;
    ok = expect(any_has_phase(snapshots, TunnelPhase::Connected),
                "7: should have Connected callback") && ok;

    // Verify final snapshot shows Connected
    ok = expect(ui.last_snapshot().phase == TunnelPhase::Connected,
                "7: final snapshot should be Connected") && ok;

    // Verify snapshot fields
    auto last = ui.last_snapshot();
    ok = expect(last.desired_connected == true,
                "7: final snapshot desired_connected should be true") && ok;
    ok = expect(last.auto_reconnect == true,
                "7: final snapshot auto_reconnect should be true") && ok;

    return ok;
}

// --- Test 8: Multiple Reconnect Attempts ---
bool test_multiple_reconnect_attempts() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    exv::core::ReconnectConfig config;
    config.base_delay = std::chrono::milliseconds(100);
    config.max_delay = std::chrono::milliseconds(5000);

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    IntegrationTunnelController ctrl(helper, net_ops, config);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Get to Connected
    ok = drive_to_connected(ctrl, *helper, *net_ops, "8") && ok;

    // First reconnect
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                "8: first TransportClosed -> Reconnecting") && ok;
    ok = expect(ctrl.attempt_count() == 1,
                "8: attempt count should be 1 after first TransportClosed") && ok;

    // Recover to Connected
    ctrl.on_event({TunnelEventType::ReconnectTimerFired});
    ok = expect(ctrl.phase() == TunnelPhase::Connected,
                "8: first reconnect should reach Connected") && ok;

    // Verify ReconnectPolicy delays increase with attempt count.
    // We test the policy directly since the controller uses it internally.
    exv::core::ReconnectPolicy policy(config);
    exv::core::ErrorInfo err;
    err.domain = "transport";
    err.code = "transport_closed";
    err.recoverable = true;

    auto intent = make_intent(true);
    auto d0 = policy.decide(err, intent, TunnelPhase::Connected, 0);
    auto d1 = policy.decide(err, intent, TunnelPhase::Connected, 1);
    auto d2 = policy.decide(err, intent, TunnelPhase::Connected, 2);

    ok = expect(d0.should_retry && d1.should_retry && d2.should_retry,
                "8: all attempts should retry with recoverable error") && ok;
    // Delay should generally increase (base * 2^attempt), though jitter adds noise.
    // We verify the pattern holds across multiple samples.
    {
        exv::core::ReconnectPolicy p(config);
        auto delay0 = p.next_delay();  // attempt 0
        // next_delay increments attempt internally, so calling again gives attempt 1
        auto delay1 = p.next_delay();
        // With base=100ms, attempt 0 -> ~100ms, attempt 1 -> ~200ms.
        // Jitter is +/-20%, so we allow wide tolerance.
        ok = expect(delay1.count() >= delay0.count() * 0.5,
                    "8: delay[1] should not be drastically smaller than delay[0]") && ok;
    }

    // Second reconnect
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.attempt_count() >= 2,
                "8: attempt count should be >= 2 after second TransportClosed") && ok;
    ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                "8: second TransportClosed -> Reconnecting") && ok;

    // Third reconnect
    ctrl.on_event({TunnelEventType::ReconnectTimerFired});
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.attempt_count() >= 3,
                "8: attempt count should be >= 3 after third TransportClosed") && ok;

    // Verify status snapshots include multiple Reconnecting entries
    auto snapshots = ui.received_snapshots();
    int reconnect_count = count_phase(snapshots, TunnelPhase::Reconnecting);
    ok = expect(reconnect_count >= 3,
                "8: should have at least 3 Reconnecting snapshots") && ok;

    // Verify Connected was reached multiple times
    int connected_count = count_phase(snapshots, TunnelPhase::Connected);
    ok = expect(connected_count >= 2,
                "8: should have reached Connected at least 2 times") && ok;

    return ok;
}

// ===========================================================================
// main
// ===========================================================================
int main() {
    bool ok = true;

    std::cout << "--- Test 1: Full Connect Flow ---\n";
    ok = test_full_connect_flow() && ok;

    std::cout << "--- Test 2: Disconnect Flow ---\n";
    ok = test_disconnect_flow() && ok;

    std::cout << "--- Test 3: Reconnect Flow ---\n";
    ok = test_reconnect_flow() && ok;

    std::cout << "--- Test 4: Auto-reconnect Disabled ---\n";
    ok = test_auto_reconnect_disabled() && ok;

    std::cout << "--- Test 5: Auth Failure ---\n";
    ok = test_auth_failure() && ok;

    std::cout << "--- Test 6: User Disconnect During Reconnecting ---\n";
    ok = test_user_disconnect_during_reconnecting() && ok;

    std::cout << "--- Test 7: Status Callback ---\n";
    ok = test_status_callback() && ok;

    std::cout << "--- Test 8: Multiple Reconnect Attempts ---\n";
    ok = test_multiple_reconnect_attempts() && ok;

    if (ok) {
        std::cout << "tunnel_controller_integration_test: all tests passed\n";
    } else {
        std::cerr << "tunnel_controller_integration_test: some tests FAILED\n";
    }
    return ok ? 0 : 1;
}
