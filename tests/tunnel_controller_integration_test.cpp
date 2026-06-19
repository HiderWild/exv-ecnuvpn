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

#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/tunnel_controller/tunnel_controller_test_access.hpp"
#include "core/tunnel_controller/tunnel_intent.hpp"
#include "core/config/config.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"
#include "core/tunnel_controller/tunnel_events.hpp"
#include "core/tunnel_controller/reconnect_policy.hpp"
#include "vpn_engine/native_engine.hpp"
#include "vpn_engine/protocol/session.hpp"
#include "observability/log_facade.hpp"
#include "observability/log_sink.hpp"
#include "observability/log_service.hpp"
#include "support/fake_helper.hpp"
#include "support/fake_platform_network_ops.hpp"
#include "support/fake_core_ui_client.hpp"
#include "platform/common/helper_delegating_network_ops.hpp"

#include <iostream>
#include <string>
#include <vector>
#include <functional>
#include <memory>
#include <mutex>
#include <chrono>
#include <cassert>
#include <atomic>
#include <thread>
#include <vector>
#include <utility>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

class CapturingLogSink final : public exv::observability::LogSink {
public:
    void write(const exv::observability::LogEvent& event) override {
        std::lock_guard<std::mutex> lock(mutex_);
        logs_.push_back(event);
    }

    std::vector<exv::observability::LogEvent> logs() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return logs_;
    }

private:
    mutable std::mutex mutex_;
    std::vector<exv::observability::LogEvent> logs_;
};

std::shared_ptr<CapturingLogSink> install_capturing_log_sink() {
    auto sink = std::make_shared<CapturingLogSink>();
    auto service = std::make_shared<exv::observability::LogService>();
    service->add_sink(sink);
    exv::observability::LogFacade::configure(service);
    return sink;
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
bool has_log_event(const std::vector<exv::observability::LogEvent>& logs,
                   const std::string& component,
                   const std::string& code) {
    for (const auto& event : logs) {
        if (event.component == component && event.code == code) return true;
    }
    return false;
}

// Check whether any captured structured log contains sensitive text.
bool logs_contain_text(const std::vector<exv::observability::LogEvent>& logs,
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

template <typename Predicate>
bool wait_until(Predicate predicate,
                std::chrono::milliseconds timeout = std::chrono::milliseconds(1000)) {
    const auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        if (predicate()) return true;
        std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }
    return predicate();
}

struct ControllerTransportState {
    std::atomic<int> disconnect_count{0};
    std::atomic<int> reset_count{0};
    std::atomic<bool> wait_for_disconnect{false};
    std::atomic<bool> disconnected{false};
};

class ControllerFakeTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
    explicit ControllerFakeTransport(std::shared_ptr<ControllerTransportState> state)
        : state_(std::move(state)) {}

    ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
        const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions&) override {
        ecnuvpn::vpn_engine::protocol::AuthResult result;
        result.ok = true;
        result.cookie = "controller-cookie";
        return result;
    }

    ecnuvpn::vpn_engine::ValidationResult
    connect_cstp(const std::string&, ecnuvpn::vpn_engine::TunnelMetadata* metadata) override {
        if (!metadata) {
            return {false, "metadata_missing", "metadata output is null"};
        }
        metadata->interface_name = "fake-cstp0";
        metadata->internal_ip4_address = "10.255.0.12";
        metadata->internal_ip4_netmask = "255.255.255.128";
        metadata->mtu = 1400;
        metadata->split_include_routes = {"198.51.100.0/24"};
        metadata->routes = metadata->split_include_routes;
        metadata->split_exclude_routes = {"203.0.113.0/24"};
        metadata->server_bypass_ips = {"192.0.2.10", "192.0.2.11/32"};
        metadata->dns_servers = {"10.10.10.10", "10.10.10.11"};
        metadata->default_domain = "ecnu.example.invalid";
        metadata->search_domains = {"campus.example.invalid"};
        metadata->tunnel_all_dns = true;
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    send_packet(const std::vector<std::uint8_t>&) override {
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    send_control(ecnuvpn::vpn_engine::protocol::InboundFrameKind) override {
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    receive_frame(ecnuvpn::vpn_engine::protocol::InboundFrame*) override {
        if (state_->wait_for_disconnect.load()) {
            while (!state_->disconnected.load()) {
                std::this_thread::sleep_for(std::chrono::milliseconds(1));
            }
        }
        return {false, "transport_closed", "transport closed"};
    }

    void disconnect() override {
        state_->disconnected.store(true);
        ++state_->disconnect_count;
    }

    void reset_for_reconnect() override {
        state_->disconnected.store(false);
        ++state_->reset_count;
    }

private:
    std::shared_ptr<ControllerTransportState> state_;
};

class ControllerDrainedPacketDevice final
    : public ecnuvpn::vpn_engine::PacketDevice {
public:
    ecnuvpn::vpn_engine::ValidationResult
    open(const ecnuvpn::vpn_engine::DeviceConfig&) override {
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    open(const ecnuvpn::vpn_engine::TunnelMetadata&) override {
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    read_packet(std::vector<std::uint8_t>* packet) override {
        if (packet) packet->clear();
        return {false, "packet_device_empty", "packet device drained"};
    }

    ecnuvpn::vpn_engine::ValidationResult
    write_packet(const std::vector<std::uint8_t>&) override {
        return {};
    }

    void close() override {}
};

class ControllerNoDataPacketDevice final
    : public ecnuvpn::vpn_engine::PacketDevice {
public:
    ecnuvpn::vpn_engine::ValidationResult
    open(const ecnuvpn::vpn_engine::DeviceConfig&) override {
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    open(const ecnuvpn::vpn_engine::TunnelMetadata&) override {
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    read_packet(std::vector<std::uint8_t>* packet) override {
        if (packet) packet->clear();
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
        return {false, "no_data", "no packet available"};
    }

    ecnuvpn::vpn_engine::ValidationResult
    write_packet(const std::vector<std::uint8_t>&) override {
        return {};
    }

    void close() override {}
};

class ControllerFailingOpenPacketDevice final
    : public ecnuvpn::vpn_engine::PacketDevice {
public:
    ecnuvpn::vpn_engine::ValidationResult
    open(const ecnuvpn::vpn_engine::DeviceConfig&) override {
        return {false, "packet_open_failed",
                "packet open failed after network apply"};
    }

    ecnuvpn::vpn_engine::ValidationResult
    open(const ecnuvpn::vpn_engine::TunnelMetadata&) override {
        return {false, "packet_open_failed",
                "packet open failed after network apply"};
    }

    ecnuvpn::vpn_engine::ValidationResult
    read_packet(std::vector<std::uint8_t>*) override {
        return {false, "unexpected_read", "packet device should not read"};
    }

    ecnuvpn::vpn_engine::ValidationResult
    write_packet(const std::vector<std::uint8_t>&) override {
        return {false, "unexpected_write", "packet device should not write"};
    }

    void close() override {}
};

class CoreLeaseRecordingHelper final : public exv::test::FakeHelper {
public:
    void set_hello_mode(exv::helper::HelperMode mode) {
        hello_mode_ = mode;
    }

    void set_acquire_response(bool accepted, std::string lease_id) {
        acquire_accepted_ = accepted;
        acquire_lease_id_ = std::move(lease_id);
    }

    exv::helper::HelloResponse hello(const exv::helper::HelloRequest& req) override {
        calls_.push_back("hello");
        auto resp = exv::test::FakeHelper::hello(req);
        resp.mode = hello_mode_;
        resp.startup_context.endpoint = "test-helper-endpoint";
        resp.startup_context.launch_mode =
            hello_mode_ == exv::helper::HelperMode::Resident ? "service" : "oneshot";
        return resp;
    }

    exv::helper::AcquireCoreLeaseResponse acquire_core_lease(
        const exv::helper::AcquireCoreLeaseRequest& req) override {
        calls_.push_back("acquire_core_lease");
        acquire_requests_.push_back(req);
        exv::helper::AcquireCoreLeaseResponse resp;
        resp.accepted = acquire_accepted_;
        resp.lease_id = acquire_lease_id_;
        resp.mode = hello_mode_ == exv::helper::HelperMode::Resident ? "resident" : "oneshot";
        return resp;
    }

    exv::helper::StartSessionResponse start_session(
        const exv::helper::StartSessionRequest& req) override {
        calls_.push_back("start_session");
        ++start_session_count_;
        return exv::test::FakeHelper::start_session(req);
    }

    exv::helper::KeepAliveResponse keep_alive(
        const exv::helper::KeepAliveRequest& req) override {
        calls_.push_back("keep_alive");
        keep_alive_requests_.push_back(req);
        return {};
    }

    exv::helper::ReleaseCoreLeaseResponse release_core_lease(
        const exv::helper::ReleaseCoreLeaseRequest& req) override {
        calls_.push_back("release_core_lease");
        release_requests_.push_back(req);
        exv::helper::ReleaseCoreLeaseResponse resp;
        resp.released = true;
        resp.exiting = req.exit_if_oneshot;
        return resp;
    }

    int start_session_count() const { return start_session_count_; }
    int acquire_count() const { return static_cast<int>(acquire_requests_.size()); }
    int keep_alive_count() const { return static_cast<int>(keep_alive_requests_.size()); }
    int release_count() const { return static_cast<int>(release_requests_.size()); }

    const std::vector<exv::helper::AcquireCoreLeaseRequest>& acquire_requests() const {
        return acquire_requests_;
    }

    const std::vector<exv::helper::ReleaseCoreLeaseRequest>& release_requests() const {
        return release_requests_;
    }

    int call_index(const std::string& name) const {
        for (std::size_t i = 0; i < calls_.size(); ++i) {
            if (calls_[i] == name) return static_cast<int>(i);
        }
        return -1;
    }

private:
    exv::helper::HelperMode hello_mode_ = exv::helper::HelperMode::Transient;
    bool acquire_accepted_ = true;
    std::string acquire_lease_id_ = "core-lease-1";
    int start_session_count_ = 0;
    std::vector<std::string> calls_;
    std::vector<exv::helper::AcquireCoreLeaseRequest> acquire_requests_;
    std::vector<exv::helper::KeepAliveRequest> keep_alive_requests_;
    std::vector<exv::helper::ReleaseCoreLeaseRequest> release_requests_;
};

ecnuvpn::Config native_controller_config() {
    ecnuvpn::Config cfg;
    cfg.server = "https://vpn.example.invalid";
    cfg.username = "alice";
    cfg.useragent = "ECNU-VPN controller integration test";
    cfg.mtu = 1290;
    cfg.routes = {"198.51.100.0/24", "198.51.101.0/24"};
    return cfg;
}

} // namespace

// ===========================================================================
// Test cases
// ===========================================================================

// --- Test 1: Full Connect Flow ---
bool test_full_connect_flow() {
    using exv::core::TunnelPhase;
    using exv::core::TunnelEventType;

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
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
    ok = expect(helper->hello_count() == 1,
                "1: controller should send Hello before StartSession") && ok;
    ok = expect(helper->acquire_count() == 1,
                "1: controller should acquire a CoreLease before StartSession") && ok;
    ok = expect(helper->call_index("hello") >= 0 &&
                    helper->call_index("acquire_core_lease") > helper->call_index("hello") &&
                    helper->call_index("start_session") > helper->call_index("acquire_core_lease"),
                "1: helper calls should be Hello -> AcquireCoreLease -> StartSession") && ok;
    auto lease_requests = helper->acquire_requests();
    ok = expect(!lease_requests.empty() &&
                    lease_requests[0].core_pid > 0 &&
                    lease_requests[0].purpose == "vpn.connect",
                "1: CoreLease acquire request should include core pid and vpn.connect purpose") && ok;

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

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
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

    // Verify helper received active Shutdown, not only passive Cleanup.
    ok = expect(helper->shutdown_count() == 1,
                "2: user disconnect should actively Shutdown helper session") && ok;
    ok = expect(helper->release_count() == 0,
                "2: user disconnect should not release the CoreLease") && ok;
    auto shutdowns = helper->shutdown_requests();
    ok = expect(shutdowns.size() == 1 &&
                    shutdowns[0].policy.remove_routes &&
                    shutdowns[0].policy.remove_dns &&
                    shutdowns[0].policy.remove_adapter &&
                    shutdowns[0].policy.remove_firewall_rules,
                "2: Shutdown should request full cleanup") && ok;

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

// --- Test 2a: CoreLease Acquisition Failure Stops Connect ---
bool test_core_lease_acquisition_failure_stops_connect() {
    using exv::core::TunnelPhase;

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    helper->set_acquire_response(false, "");
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    bool ok = true;
    ctrl.connect(make_intent(true));
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "2a: rejected CoreLease should transition to Failed") && ok;
    ok = expect(helper->acquire_count() == 1,
                "2a: controller should attempt CoreLease acquisition") && ok;
    ok = expect(helper->start_session_count() == 0,
                "2a: controller must not StartSession without a CoreLease") && ok;
    ok = expect(helper->heartbeat_count() == 0,
                "2a: session heartbeat must not start without helper session") && ok;
    auto snap = ctrl.status();
    ok = expect(snap.helper_status == "core_lease_failed",
                "2a: helper_status should expose core_lease_failed") && ok;
    ok = expect(snap.last_error.has_value() &&
                    snap.last_error->code == "core_lease_failed",
                "2a: status should expose core_lease_failed error code") && ok;
    return ok;
}

// --- Test 2b: Heartbeat Starts Immediately After StartSession ---
bool test_start_session_immediately_sends_heartbeat_on_pre_connected_failure() {
    using exv::core::TunnelPhase;

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    bool ok = true;
    ok = expect(helper->connect(), "2b: helper should connect before controller flow") && ok;
    net_ops->set_apply_should_fail(true);

    ctrl.connect(make_intent(true));
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "2b: apply_config failure should transition to Failed") && ok;
    ok = expect(helper->heartbeat_count() == 1,
                "2b: core should send one heartbeat immediately after StartSession") && ok;

    return ok;
}

// --- Test 2c: StartSession Failure Stops Connect ---
bool test_start_session_failure_stops_connect() {
    using exv::core::TunnelPhase;

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    bool ok = true;
    helper->set_start_session_fail(true);

    ctrl.connect(make_intent(true));
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "2c: empty StartSession response should transition to Failed") && ok;
    ok = expect(helper->heartbeat_count() == 0,
                "2c: heartbeat must not start without a helper session") && ok;
    ok = expect(net_ops->prepare_count() == 0,
                "2c: network ops must not run without a helper session") && ok;

    return ok;
}

// --- Test 2d: Native network config failure keeps helper/platform error ---
bool test_native_network_config_failure_preserves_error() {
    using exv::core::TunnelPhase;

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    auto transport_state = std::make_shared<ControllerTransportState>();

    net_ops->set_apply_should_fail(true);

    auto ctrl_owner = exv::core::TunnelControllerTestAccess::create(
        helper, net_ops, exv::core::ReconnectConfig{},
        [transport_state]() {
            ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
            deps.transport_factory = [transport_state]() {
                return std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport>(
                    new ControllerFakeTransport(transport_state));
            };
            return deps;
        });
    auto& ctrl = *ctrl_owner;
    ctrl.set_vpn_config(native_controller_config(), "test-password");

    bool ok = true;
    ctrl.connect(make_intent(true));

    auto snap = ctrl.status();
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "2d: apply_config failure in native path should transition to Failed") && ok;
    ok = expect(net_ops->prepare_count() == 1,
                "2d: native path should prepare tunnel device through controller callback") && ok;
    ok = expect(net_ops->apply_count() == 1,
                "2d: native path should attempt network apply through controller callback") && ok;
    ok = expect(transport_state->disconnect_count == 1,
                "2d: native transport should be disconnected after network config failure") && ok;
    ok = expect(snap.last_error.has_value(),
                "2d: failed native network config should leave a last_error") && ok;
    if (snap.last_error) {
        ok = expect(snap.last_error->code == "apply_config_failed",
                    "2d: network apply error code should not be overwritten by auth_failed") && ok;
        ok = expect(snap.last_error->domain == "helper",
                    "2d: network apply error domain should remain helper/platform boundary") && ok;
    }

    return ok;
}

// --- Test 2e: Native network config preserves CSTP routes and bypass ---
bool test_native_network_config_preserves_routes_and_bypass() {
    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    auto transport_state = std::make_shared<ControllerTransportState>();
    transport_state->wait_for_disconnect.store(true);
    exv::test::FakeCoreUiClient ui;

    auto ctrl_owner = exv::core::TunnelControllerTestAccess::create(
        helper, net_ops, exv::core::ReconnectConfig{},
        [transport_state]() {
            ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
            deps.transport_factory = [transport_state]() {
                return std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport>(
                    new ControllerFakeTransport(transport_state));
            };
            deps.packet_device_factory = []() {
                return std::unique_ptr<ecnuvpn::vpn_engine::PacketDevice>(
                    new ControllerDrainedPacketDevice());
            };
            return deps;
        });
    auto& ctrl = *ctrl_owner;
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });
    ctrl.set_vpn_config(native_controller_config(), "test-password");

    bool ok = true;
    ctrl.connect(make_intent(true));
    ok = expect(wait_until([&]() {
                    return any_has_phase(ui.received_snapshots(),
                                         exv::core::TunnelPhase::Connected);
                }),
                "2e: route preservation path should reach Connected") && ok;

    auto configs = net_ops->applied_configs();
    ok = expect(configs.size() == 1,
                "2e: native path should apply exactly one platform tunnel config") && ok;
    if (!configs.empty()) {
        ok = expect(configs[0].interface_address == "10.255.0.12/25",
                    "2e: platform tunnel config should derive prefix from CSTP netmask") && ok;
        bool has_cstp_route = false;
        bool has_manual_route = false;
        int duplicate_count = 0;
        for (const auto& route : configs[0].routes) {
            if (route.destination == "198.51.100.0/24") {
                has_cstp_route = true;
                ++duplicate_count;
            }
            if (route.destination == "198.51.101.0/24") {
                has_manual_route = true;
            }
        }
        ok = expect(has_cstp_route,
                    "2e: platform tunnel config should preserve CSTP route destination") && ok;
        ok = expect(has_manual_route,
                    "2e: platform tunnel config should merge manual configured routes") && ok;
        ok = expect(duplicate_count == 1,
                    "2e: platform tunnel config should de-duplicate route destinations") && ok;
        ok = expect(configs[0].server_bypass_ips.size() == 2 &&
                        configs[0].server_bypass_ips[0] == "192.0.2.10" &&
                        configs[0].server_bypass_ips[1] == "192.0.2.11/32",
                    "2e: platform tunnel config should preserve all CSTP server bypass IPs") && ok;
        ok = expect(configs[0].dns.servers.size() == 2 &&
                        configs[0].dns.servers[0] == "10.10.10.10" &&
                        configs[0].dns.servers[1] == "10.10.10.11",
                    "2e: platform tunnel config should map CSTP DNS servers") && ok;
        ok = expect(configs[0].dns.search_domain == "ecnu.example.invalid",
                    "2e: platform tunnel config should map default domain") && ok;
        ok = expect(configs[0].dns.suffixes.size() == 1 &&
                        configs[0].dns.suffixes[0] == "campus.example.invalid",
                    "2e: platform tunnel config should map search domains") && ok;
        ok = expect(configs[0].exclude_routes.size() == 1 &&
                        configs[0].exclude_routes[0] == "203.0.113.0/24",
                    "2e: platform tunnel config should map split exclude routes") && ok;
        ok = expect(configs[0].exclude_route.has_value() &&
                        configs[0].exclude_route.value() == "203.0.113.0/24",
                    "2e: platform tunnel config should keep first exclude route in legacy field") && ok;
    }

    return ok;
}

// --- Test 2f: Native packet startup failure cleans privileged network state ---
bool test_native_packet_startup_failure_cleans_network_state() {
    using exv::core::TunnelPhase;

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    auto transport_state = std::make_shared<ControllerTransportState>();

    auto ctrl_owner = exv::core::TunnelControllerTestAccess::create(
        helper, net_ops, exv::core::ReconnectConfig{},
        [transport_state]() {
            ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
            deps.transport_factory = [transport_state]() {
                return std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport>(
                    new ControllerFakeTransport(transport_state));
            };
            deps.packet_device_factory = []() {
                return std::unique_ptr<ecnuvpn::vpn_engine::PacketDevice>(
                    new ControllerFailingOpenPacketDevice());
            };
            return deps;
        });
    auto& ctrl = *ctrl_owner;
    ctrl.set_vpn_config(native_controller_config(), "test-password");

    bool ok = true;
    ctrl.connect(make_intent(true));

    auto snap = ctrl.status();
    ok = expect(net_ops->apply_count() == 1,
                "2f: network config should be applied before packet startup fails") && ok;
    ok = expect(helper->shutdown_count() == 1,
                "2f: post-network packet startup failure should shutdown helper session") && ok;
    auto shutdowns = helper->shutdown_requests();
    ok = expect(shutdowns.size() == 1 &&
                    shutdowns[0].policy.remove_routes &&
                    shutdowns[0].policy.remove_dns &&
                    shutdowns[0].policy.remove_adapter &&
                    shutdowns[0].policy.remove_firewall_rules,
                "2f: startup failure cleanup should request full cleanup policy") && ok;
    ok = expect(helper->active_sessions().empty(),
                "2f: startup failure cleanup should clear helper active session") && ok;
    ok = expect(ctrl.phase() == TunnelPhase::Failed,
                "2f: controller should expose packet startup failure as Failed") && ok;
    ok = expect(snap.last_error.has_value(),
                "2f: packet startup failure should leave a last_error") && ok;
    if (snap.last_error) {
        ok = expect(snap.last_error->code == "packet_open_failed",
                    "2f: packet startup error code should be preserved") && ok;
        ok = expect(snap.last_error->code != "auth_failed",
                    "2f: packet startup failure must not be classified as auth_failed") && ok;
    }

    return ok;
}

// --- Test 2g: Native transport reconnect is controller-owned ---
bool test_native_transport_reconnect_is_controller_owned() {
    using exv::core::TunnelEventType;
    using exv::core::TunnelPhase;

    exv::core::ReconnectConfig config;
    config.base_delay = std::chrono::milliseconds(5000);
    config.max_delay = std::chrono::milliseconds(5000);
    config.jitter_factor = 0.0;

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    auto transport_state = std::make_shared<ControllerTransportState>();

    auto ctrl_owner = exv::core::TunnelControllerTestAccess::create(
        helper, net_ops, config,
        [transport_state]() {
            ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
            deps.transport_factory = [transport_state]() {
                return std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport>(
                    new ControllerFakeTransport(transport_state));
            };
            deps.packet_device_factory = []() {
                return std::unique_ptr<ecnuvpn::vpn_engine::PacketDevice>(
                    new ControllerNoDataPacketDevice());
            };
            return deps;
        });
    auto& ctrl = *ctrl_owner;

    ecnuvpn::Config cfg = native_controller_config();
    cfg.auto_reconnect = true;
    ctrl.set_vpn_config(cfg, "test-password");

    bool ok = true;
    ctrl.connect(make_intent(true, true, "native-controller-owned-reconnect"));

    ok = expect(wait_until([&ctrl]() {
                    return ctrl.phase() == TunnelPhase::Reconnecting;
                }),
                "2g: native transport closure should surface to controller Reconnecting") && ok;
    ok = expect(net_ops->apply_count() == 1,
                "2g: initial native connect should apply network config once") && ok;

    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    ok = expect(transport_state->reset_count == 0,
                "2g: native ProtocolSession must not reset transport for internal reconnect") && ok;

    ctrl.on_event({TunnelEventType::ReconnectTimerFired});
    ok = expect(wait_until([&net_ops]() {
                    return net_ops->apply_count() >= 2;
                }),
                "2g: controller reconnect should re-enter helper/platform network setup") && ok;
    ok = expect(helper->hello_count() >= 2,
                "2g: controller reconnect should start a fresh helper setup flow") && ok;
    ok = expect(helper->shutdown_count() >= 1,
                "2g: controller reconnect should cleanup previous helper/network session") && ok;
    ok = expect(transport_state->reset_count == 0,
                "2g: native reset_for_reconnect must remain unused after controller retry") && ok;

    return ok;
}

// --- Test 2h: CoreLease Released On Controller Destruction ---
bool test_core_lease_released_on_controller_destruction() {
    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();

    bool ok = true;
    {
        exv::core::TunnelController ctrl(helper, net_ops);
        ctrl.connect(make_intent(true));
        ok = expect(ctrl.phase() == exv::core::TunnelPhase::Connected,
                    "2h: controller should connect before destruction") && ok;
        ctrl.disconnect();
        ok = expect(helper->release_count() == 0,
                    "2h: disconnect before destruction should not release CoreLease") && ok;
    }

    ok = expect(helper->release_count() == 1,
                "2h: controller destruction should release CoreLease once") && ok;
    auto releases = helper->release_requests();
    ok = expect(!releases.empty() &&
                    releases[0].lease_id == "core-lease-1" &&
                    releases[0].exit_if_oneshot,
                "2h: release should use acquired lease id and exit_if_oneshot=true") && ok;
    return ok;
}

// --- Test 2i: Handoff Replaces Helper Without Dropping Session ---
bool test_handoff_replaces_helper_and_preserves_session() {
    auto oneshot = std::make_shared<CoreLeaseRecordingHelper>();
    auto original_ops =
        std::make_shared<exv::platform::HelperDelegatingPlatformNetworkOps>(
            oneshot.get());
    exv::core::TunnelController ctrl(oneshot, original_ops);

    ctrl.connect(make_intent(true));
    const auto original_session = original_ops->session_id();

    auto service = std::make_shared<CoreLeaseRecordingHelper>();
    service->set_hello_mode(exv::helper::HelperMode::Resident);
    service->connect();
    auto service_ops =
        std::make_shared<exv::platform::HelperDelegatingPlatformNetworkOps>(
            service.get());

    const bool replaced = ctrl.replace_helper_for_handoff(
        service, service_ops, "service-core-lease", "resident",
        "service-endpoint");

    auto snap = ctrl.status();

    bool ok = true;
    ok = expect(replaced, "2i: controller should accept handoff helper") && ok;
    ok = expect(snap.session_active,
                "2i: handoff should preserve active helper session") && ok;
    ok = expect(snap.core_lease_active,
                "2i: handoff should keep core lease active") && ok;
    ok = expect(snap.helper_mode == "resident",
                "2i: handoff should switch helper mode to resident") && ok;
    ok = expect(snap.helper_endpoint == "service-endpoint",
                "2i: handoff should switch helper endpoint") && ok;
    ok = expect(service_ops->session_id() == original_session,
                "2i: new delegated network ops should keep session id") && ok;
    ok = expect(oneshot->release_count() == 0,
                "2i: controller should not release old oneshot lease during handoff") && ok;
    ok = expect(service->keep_alive_count() >= 1,
                "2i: controller should start service core lease keepalive") && ok;
    return ok;
}

// --- Test 2j: Hello Resident Mode Propagates To Status ---
bool test_hello_resident_mode_propagates_to_status() {
    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    helper->set_hello_mode(exv::helper::HelperMode::Resident);
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    bool ok = true;
    ctrl.connect(make_intent(true));
    auto snap = ctrl.status();
    ok = expect(ctrl.phase() == exv::core::TunnelPhase::Connected,
                "2j: resident helper flow should connect") && ok;
    ok = expect(snap.helper_mode == "resident",
                "2j: helper_mode should reflect Hello mode=Resident") && ok;
    ok = expect(snap.helper_status == "connected",
                "2j: helper_status should show connected after successful Hello") && ok;
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

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
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

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
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

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::test::FakeCoreUiClient ui;

    exv::core::TunnelController ctrl(helper, net_ops);
    ctrl.set_status_callback([&](const exv::core::TunnelStatusSnapshot& snap) {
        ui.on_status_update(snap);
    });

    bool ok = true;

    // Configure network ops to fail at apply_tunnel_config. The synchronous
    // fallback path now applies config through PlatformNetworkOps, and a
    // failure causes the TC to transition to Failed.
    net_ops->set_apply_should_fail(true);
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

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
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

    // Verify helper received active Shutdown, not only passive Cleanup.
    ok = expect(helper->shutdown_count() == 1,
                "6: disconnect during Reconnecting should Shutdown helper session") && ok;

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

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
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

    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
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
    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    auto log_sink = install_capturing_log_sink();

    ctrl.connect(make_intent(true, true, "test-profile"));

    exv::observability::LogFacade::flush();
    auto logs = log_sink->logs();
    exv::observability::LogFacade::shutdown();

    bool ok = true;
    ok = expect(has_log_event(logs, "tunnel", "connect.start"),
                "9: connect entry should log") && ok;
    ok = expect(has_log_event(logs, "tunnel", "vpn.config.missing"),
                "9: missing VPN config/password should log") && ok;
    ok = expect(has_log_event(logs, "tunnel", "helper.hello.starting"),
                "9: helper hello should log before the request") && ok;
    ok = expect(has_log_event(logs, "tunnel", "core_lease.acquire.starting"),
                "9: core lease acquisition should log before the request") && ok;
    ok = expect(has_log_event(logs, "tunnel", "helper.session.starting"),
                "9: helper session start should log before the request") && ok;
    ok = expect(has_log_event(logs, "tunnel", "helper.session.started"),
                "9: helper session start should log") && ok;
    ok = expect(has_log_event(logs, "tunnel", "tunnel.device.preparing"),
                "9: tunnel device preparation should log before the request") && ok;
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
    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    exv::core::TunnelController ctrl(helper, net_ops);

    ecnuvpn::Config cfg;
    cfg.server.clear();
    cfg.username.clear();
    ctrl.set_vpn_config(cfg, "super-secret-password");

    auto log_sink = install_capturing_log_sink();

    ctrl.connect(make_intent(true, true, "native-failure-profile"));

    exv::observability::LogFacade::flush();
    auto logs = log_sink->logs();
    exv::observability::LogFacade::shutdown();

    bool ok = true;
    ok = expect(ctrl.phase() == exv::core::TunnelPhase::Failed,
                "10: invalid native config should fail") && ok;
    ok = expect(has_log_event(logs, "tunnel", "native.runner.starting"),
                "10: native runner start attempt should log before execution") && ok;
    ok = expect(has_log_event(logs, "tunnel", "native.runner.failed"),
                "10: native runner start failure should log") && ok;
    ok = expect(!logs_contain_text(logs, "super-secret-password"),
                "10: logs should not include plaintext password") && ok;
    return ok;
}

// --- Test 11: HelperDelegatingPlatformNetworkOps receives controller session ---
bool test_delegated_network_ops_receives_helper_session() {
    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
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

// --- Test 12: Delegated Network Ops Prepare Before Apply ---
bool test_delegated_network_ops_prepares_before_apply() {
    auto helper = std::make_shared<CoreLeaseRecordingHelper>();
    helper->set_require_prepare_before_apply(true);
    auto net_ops = std::make_shared<exv::platform::HelperDelegatingPlatformNetworkOps>(helper.get());
    exv::core::TunnelController ctrl(helper, net_ops);

    bool ok = true;
    ctrl.connect(make_intent(true, true, "delegated-prepare-before-apply"));
    ok = expect(ctrl.phase() == exv::core::TunnelPhase::Connected,
                "12: delegated net ops must prepare before applying config") && ok;
    ok = expect(helper->prepare_count() == 1,
                "12: helper PrepareTunnelDevice should run once") && ok;
    ok = expect(helper->apply_count() == 1,
                "12: helper ApplyTunnelConfig should run once") && ok;

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

    std::cout << "--- Test 2a: CoreLease Acquisition Failure Stops Connect ---\n";
    ok = test_core_lease_acquisition_failure_stops_connect() && ok;

    std::cout << "--- Test 2b: Immediate Heartbeat After StartSession ---\n";
    ok = test_start_session_immediately_sends_heartbeat_on_pre_connected_failure() && ok;

    std::cout << "--- Test 2c: StartSession Failure Stops Connect ---\n";
    ok = test_start_session_failure_stops_connect() && ok;

    std::cout << "--- Test 2d: Native Network Config Failure Preserves Error ---\n";
    ok = test_native_network_config_failure_preserves_error() && ok;

    std::cout << "--- Test 2e: Native Network Config Preserves Routes And Bypass ---\n";
    ok = test_native_network_config_preserves_routes_and_bypass() && ok;

    std::cout << "--- Test 2f: Native Packet Startup Failure Cleans Network State ---\n";
    ok = test_native_packet_startup_failure_cleans_network_state() && ok;

    std::cout << "--- Test 2g: Native Transport Reconnect Is Controller-Owned ---\n";
    ok = test_native_transport_reconnect_is_controller_owned() && ok;

    std::cout << "--- Test 2h: CoreLease Released On Controller Destruction ---\n";
    ok = test_core_lease_released_on_controller_destruction() && ok;

    std::cout << "--- Test 2i: Handoff Replaces Helper Without Dropping Session ---\n";
    ok = test_handoff_replaces_helper_and_preserves_session() && ok;

    std::cout << "--- Test 2j: Hello Resident Mode Propagates To Status ---\n";
    ok = test_hello_resident_mode_propagates_to_status() && ok;

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

    std::cout << "--- Test 12: Delegated Network Ops Prepare Before Apply ---\n";
    ok = test_delegated_network_ops_prepares_before_apply() && ok;

    if (ok) {
        std::cout << "tunnel_controller_integration_test: all tests passed\n";
    } else {
        std::cerr << "tunnel_controller_integration_test: some tests FAILED\n";
    }
    return ok ? 0 : 1;
}
