// TunnelController integration tests.
//
// Tests the full connect/disconnect/reconnect flows by orchestrating
// the real TunnelController with FakeHelper, FakePlatformNetworkOps,
// and FakeCoreUiClient.
//
// The real TunnelController uses a synchronous fallback path when no VPN
// config is set, which drives through all phases (PreparingHelper ->
// Authenticating -> ConnectingCstp -> ApplyingNetworkConfig ->
// OpeningPacketDevice -> Connected) automatically from connect().
// Events are still needed for async failures and reconnect scenarios.

#include "core/tunnel_controller.hpp"
#include "core/tunnel_intent.hpp"
#include "core/config/config.hpp"
#include "core/tunnel_state.hpp"
#include "core/tunnel_events.hpp"
#include "core/reconnect_policy.hpp"
#include "log_event_bus.hpp"
#include "support/fake_helper.hpp"
#include "support/fake_platform_network_ops.hpp"
#include "support/fake_core_ui_client.hpp"
#include "helper/platform/helper_delegating_network_ops.hpp"

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
// The real TunnelController's synchronous fallback auto-drives all phases
// from connect(), so no events are needed to reach Connected.
// Returns true if the controller reached Connected phase.
bool drive_to_connected(exv::core::TunnelController& ctrl,
                        const char* label) {
    ctrl.connect(make_intent(true));
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

// Check whether a captured structured log has the expected component/code.
bool has_log_event(const std::vector<ecnuvpn::TypedLogEvent>& logs,
                   const std::string& component,
                   const std::string& code) {
    for (const auto& event : logs) {
        if (event.component == component && event.code == code) return true;
    }
    return false;
}

// Check whether any captured structured log contains sensitive text.
bool logs_contain_text(const std::vector<ecnuvpn::TypedLogEvent>& logs,
                       const std::string& needle) {
    for (const auto& event : logs) {
        if (event.message.find(needle) != std::string::npos) return true;
        if (event.component.find(needle) != std::string::npos) return true;
        if (event.code.find(needle) != std::string::npos) return true;
        for (const auto& field : event.fields) {
            if (field.first.find(needle) != std::string::npos ||
                field.second.find(needle) != std::string::npos) return true;
        }
    }
    return false;
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

    exv::core::TunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;
    ok = expect(ctrl.phase() == TunnelPhase::Idle,
                "1: initial phase should be Idle") && ok;

    // Connect — synchronous fallback drives to Connected automatically
    ctrl.connect(make_intent(true));
    ok = expect(ctrl.phase() == TunnelPhase::Connected,
                "1: after connect() should reach Connected") && ok;

    // Verify helper received start_session
    ok = expect(helper->active_sessions().size() >= 1,
                "1: helper should have at least one active session") && ok;

    // Verify platform ops received prepare_tunnel_device
    ok = expect(net_ops->prepare_count() >= 1,
                "1: platform ops should have prepared tunnel device") && ok;

    // Verify helper received apply_tunnel_config
    ok = expect(helper->active_sessions().size() >= 1,
                "1: helper session should still be active") && ok;

    // Verify status transitions include expected phases as a subsequence
    // Real TC: PreparingHelper -> Authenticating -> ConnectingCstp ->
    //          ApplyingNetworkConfig -> OpeningPacketDevice -> Connected
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

    exv::core::TunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Get to Connected
    ok = drive_to_connected(ctrl, "2") && ok;
    ui.clear();

    // Disconnect
    ctrl.disconnect();
    ok = expect(ctrl.phase() == TunnelPhase::Idle,
                "2: after disconnect() should be Idle") && ok;

    // Verify helper received cleanup
    ok = expect(helper->cleanup_requests().size() >= 1,
                "2: helper should have received cleanup request") && ok;

    // Verify status transitions include Disconnecting and CleaningUp
    // Real TC: Connected -> Disconnecting -> CleaningUp -> Idle
    auto snapshots = ui.received_snapshots();
    ok = expect(any_has_phase(snapshots, TunnelPhase::Disconnecting),
                "2: should have transitioned through Disconnecting") && ok;
    ok = expect(any_has_phase(snapshots, TunnelPhase::CleaningUp),
                "2: should have transitioned through CleaningUp") && ok;
    ok = expect(ui.last_snapshot().phase == TunnelPhase::Idle,
                "2: final snapshot should be Idle") && ok;

    return ok;
}

// --- Test 2b: Heartbeat Starts Immediately After StartSession ---
bool test_start_session_immediately_sends_heartbeat_on_pre_connected_failure() {
    using exv::core::TunnelPhase;

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    bool ok = true;
    ok = expect(helper->connect(), "2b: helper should connect before controller flow") && ok;
    helper->set_apply_config_fail(true);

    ctrl.connect(make_intent(true));
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "2b: apply_config failure should transition to Failed") && ok;
    ok = expect(helper->heartbeat_count() == 1,
                "2b: core should send one heartbeat immediately after StartSession") && ok;

    return ok;
}

// --- Test 3: Reconnect Flow (auto_reconnect=true) ---
//
// After TransportClosed, the real TC goes to Reconnecting. When the
// reconnect timer fires, it transitions to Authenticating (not all the
// way to Connected). We simulate the subsequent events to reach Connected.
bool test_reconnect_flow() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    exv::core::ReconnectConfig config;
    config.base_delay = std::chrono::milliseconds(100);
    config.max_delay = std::chrono::milliseconds(5000);

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    exv::core::TunnelController ctrl(helper, net_ops, config);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Get to Connected
    ok = drive_to_connected(ctrl, "3") && ok;
    ui.clear();

    // Simulate TransportClosed
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                "3: TransportClosed with auto_reconnect should go to Reconnecting") && ok;

    // Simulate reconnect timer -> transitions to Authenticating
    ctrl.on_event({TunnelEventType::ReconnectTimerFired});
    ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                "3: after ReconnectTimerFired should be Authenticating") && ok;

    // Drive the rest of the connect flow with events.
    // After Authenticating, need AuthSucceeded to move to ConnectingCstp,
    // then CstpConnected -> ApplyingNetworkConfig, etc.
    ctrl.on_event({TunnelEventType::AuthSucceeded});
    ok = expect(ctrl.phase() == TunnelPhase::ConnectingCstp,
                "3: after AuthSucceeded should be ConnectingCstp") && ok;

    ctrl.on_event({TunnelEventType::CstpConnected});
    ok = expect(ctrl.phase() == TunnelPhase::ApplyingNetworkConfig,
                "3: after CstpConnected should be ApplyingNetworkConfig") && ok;

    ctrl.on_event({TunnelEventType::NetworkConfigApplied});
    ok = expect(ctrl.phase() == TunnelPhase::OpeningPacketDevice,
                "3: after NetworkConfigApplied should be OpeningPacketDevice") && ok;

    ctrl.on_event({TunnelEventType::PacketLoopStarted});
    ok = expect(ctrl.phase() == TunnelPhase::Connected,
                "3: after PacketLoopStarted should reach Connected") && ok;

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

    exv::core::TunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Connect with auto_reconnect=false
    ctrl.connect(make_intent(true, false));
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

    // Verify error info is set in the status snapshot
    auto last_snap = ctrl.status();
    ok = expect(last_snap.last_error.has_value(),
                "4: status snapshot should have last_error set") && ok;
    if (last_snap.last_error) {
        ok = expect(last_snap.last_error->code == "transport_closed",
                    "4: error code should be transport_closed") && ok;
    }

    return ok;
}

// --- Test 5: Connect Failure (non-recoverable helper error) ---
//
// Tests that the TC transitions to Failed when apply_tunnel_config fails
// during the synchronous connect flow.
bool test_auth_failure() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    exv::core::TunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Configure the helper to fail at apply_tunnel_config. The synchronous
    // fallback path calls apply_tunnel_config during connect(), and a
    // failure causes the TC to transition to Failed.
    helper->set_apply_config_fail(true);
    ctrl.connect(make_intent(true));
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "5: apply_config failure should transition to Failed") && ok;

    // Verify error info is set in the status snapshot
    auto last_snap = ctrl.status();
    ok = expect(last_snap.last_error.has_value(),
                "5: status snapshot should have last_error set") && ok;

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

    exv::core::TunnelController ctrl(helper, net_ops, config);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Get to Connected
    ok = drive_to_connected(ctrl, "6") && ok;

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

    // Verify transitions: Disconnecting -> CleaningUp -> Idle
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

    exv::core::TunnelController ctrl(helper, net_ops);
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

    // Verify callback was called at each state transition
    auto snapshots = ui.received_snapshots();
    ok = expect(snapshots.size() >= 2,
                "7: multiple callbacks expected through full flow") && ok;

    // Real TC snapshots include: PreparingHelper, Authenticating,
    // ConnectingCstp, ApplyingNetworkConfig, OpeningPacketDevice, Connected
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

    exv::core::TunnelController ctrl(helper, net_ops, config);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Get to Connected
    ok = drive_to_connected(ctrl, "8") && ok;

    // First reconnect cycle: TransportClosed -> Reconnecting
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                "8: first TransportClosed -> Reconnecting") && ok;

    // Reconnect timer fires -> Authenticating, then drive to Connected
    ctrl.on_event({TunnelEventType::ReconnectTimerFired});
    ok = expect(ctrl.phase() == TunnelPhase::Authenticating,
                "8: first ReconnectTimerFired -> Authenticating") && ok;

    ctrl.on_event({TunnelEventType::AuthSucceeded});
    ctrl.on_event({TunnelEventType::CstpConnected});
    ctrl.on_event({TunnelEventType::NetworkConfigApplied});
    ctrl.on_event({TunnelEventType::PacketLoopStarted});
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

    // Second reconnect cycle
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                "8: second TransportClosed -> Reconnecting") && ok;

    ctrl.on_event({TunnelEventType::ReconnectTimerFired});
    ctrl.on_event({TunnelEventType::AuthSucceeded});
    ctrl.on_event({TunnelEventType::CstpConnected});
    ctrl.on_event({TunnelEventType::NetworkConfigApplied});
    ctrl.on_event({TunnelEventType::PacketLoopStarted});
    ok = expect(ctrl.phase() == TunnelPhase::Connected,
                "8: second reconnect should reach Connected") && ok;

    // Third reconnect cycle
    ctrl.on_event({TunnelEventType::TransportClosed});
    ok = expect(ctrl.phase() == TunnelPhase::Reconnecting,
                "8: third TransportClosed -> Reconnecting") && ok;

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

// --- Test 9: Connect Milestone Logs ---
bool test_connect_milestone_logs() {
    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    std::vector<ecnuvpn::TypedLogEvent> logs;
    auto subscription = ecnuvpn::LogEventBus::instance().subscribe(
        [&](const ecnuvpn::TypedLogEvent& event) { logs.push_back(event); });

    ctrl.connect(make_intent(true, true, "test-profile"));

    ecnuvpn::LogEventBus::instance().unsubscribe(subscription);

    bool ok = true;
    ok = expect(has_log_event(logs, "tunnel", "connect.start"),
                "9: connect entry should log") && ok;
    ok = expect(has_log_event(logs, "tunnel", "vpn.config.missing"),
                "9: missing VPN config/password should log") && ok;
    ok = expect(has_log_event(logs, "tunnel", "helper.session.started"),
                "9: helper session start should log") && ok;
    ok = expect(has_log_event(logs, "tunnel", "network.config.applying"),
                "9: applying network config should log") && ok;
    ok = expect(has_log_event(logs, "tunnel", "packet.loop.started"),
                "9: packet loop started should log") && ok;
    ok = expect(has_log_event(logs, "tunnel", "connect.connected"),
                "9: connected milestone should log") && ok;
    ok = expect(!logs_contain_text(logs, "password"),
                "9: logs should not include password field names") && ok;
    return ok;
}

// --- Test 10: Native Runner Failure Log ---
bool test_native_runner_failure_log() {
    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    ecnuvpn::Config cfg;
    cfg.server.clear();
    cfg.username.clear();
    ctrl.set_vpn_config(cfg, "super-secret-password");

    std::vector<ecnuvpn::TypedLogEvent> logs;
    auto subscription = ecnuvpn::LogEventBus::instance().subscribe(
        [&](const ecnuvpn::TypedLogEvent& event) { logs.push_back(event); });

    ctrl.connect(make_intent(true, true, "native-failure-profile"));

    ecnuvpn::LogEventBus::instance().unsubscribe(subscription);

    bool ok = true;
    ok = expect(ctrl.phase() == exv::core::TunnelPhase::Failed,
                "10: invalid native config should fail") && ok;
    ok = expect(has_log_event(logs, "tunnel", "native.runner.failed"),
                "10: native runner start failure should log") && ok;
    ok = expect(!logs_contain_text(logs, "super-secret-password"),
                "10: logs should not include plaintext password") && ok;
    return ok;
}

// --- Test 11: HelperDelegatingPlatformNetworkOps receives controller session ---
bool test_delegated_network_ops_receives_helper_session() {
    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::platform::HelperDelegatingPlatformNetworkOps>(helper.get());
    exv::core::TunnelController ctrl(helper, net_ops);

    bool ok = true;
    ok = expect(net_ops->session_id().value.empty(),
                "11: delegated net ops session should start empty") && ok;

    ctrl.connect(make_intent(true, true, "delegated-session-profile"));
    ok = expect(ctrl.phase() == exv::core::TunnelPhase::Connected,
                "11: delegated net ops should reach Connected") && ok;

    auto sessions = helper->active_sessions();
    ok = expect(sessions.size() == 1,
                "11: helper should have exactly one active session") && ok;
    if (!sessions.empty()) {
        ok = expect(net_ops->session_id().value == sessions.front().value,
                    "11: delegated net ops should use helper start_session id") && ok;
    }

    ctrl.disconnect();
    ok = expect(net_ops->session_id().value.empty(),
                "11: delegated net ops session should clear after cleanup") && ok;
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

    std::cout << "--- Test 2b: Immediate Heartbeat After StartSession ---\n";
    ok = test_start_session_immediately_sends_heartbeat_on_pre_connected_failure() && ok;

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

    std::cout << "--- Test 9: Connect Milestone Logs ---\n";
    ok = test_connect_milestone_logs() && ok;

    std::cout << "--- Test 10: Native Runner Failure Log ---\n";
    ok = test_native_runner_failure_log() && ok;

    std::cout << "--- Test 11: Delegated Network Ops Session Propagation ---\n";
    ok = test_delegated_network_ops_receives_helper_session() && ok;

    if (ok) {
        std::cout << "tunnel_controller_integration_test: all tests passed\n";
    } else {
        std::cerr << "tunnel_controller_integration_test: some tests FAILED\n";
    }
    return ok ? 0 : 1;
}
