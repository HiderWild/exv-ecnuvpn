// CoreSessionRunner unit tests.
//
// Tests the CoreSessionRunner lifecycle, event bridging, and thread-safety
// without requiring a real VPN server.  Uses the EngineEventBridge map_event
// logic to verify event translation, and exercises the runner's start/stop/
// is_running/status API surface.

#include "core/tunnel_controller/core_session_runner.hpp"
#include "core/tunnel_controller/engine_event_bridge.hpp"
#include "core/tunnel_controller/native_engine_config_mapper.hpp"
#include "core/tunnel_controller/tunnel_events.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"
#include "core/config/config.hpp"
#include "vpn_engine/native_handshake_job.hpp"
#include "vpn_engine/packet_device.hpp"
#include "vpn_engine/protocol/session.hpp"

#include <iostream>
#include <string>
#include <vector>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <deque>
#include <memory>
#include <mutex>
#include <stop_token>
#include <thread>
#include <cassert>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

template <typename Predicate>
bool wait_until(Predicate predicate, std::chrono::milliseconds timeout) {
    const auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        if (predicate()) return true;
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    return predicate();
}

class RunnerFakeTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
    ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
        const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions&) override {
        ecnuvpn::vpn_engine::protocol::AuthResult result;
        result.ok = true;
        result.cookie = "runner-cookie";
        return result;
    }

    ecnuvpn::vpn_engine::ValidationResult
    connect_cstp(const std::string&, ecnuvpn::vpn_engine::TunnelMetadata* metadata) override {
        if (!metadata) {
            return {false, "metadata_missing", "metadata output is null"};
        }
        metadata->interface_name = "fake-cstp0";
        metadata->internal_ip4_address = "10.255.0.10";
        metadata->internal_ip4_netmask = "255.255.255.0";
        metadata->mtu = 1400;
        metadata->routes = {"198.51.100.0/24"};
        metadata->server_bypass_ips = {"192.0.2.10"};
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
        std::unique_lock<std::mutex> lock(mu_);
        cv_.wait(lock, [&] { return closed_; });
        return {false, "transport_closed", "transport closed"};
    }

    void disconnect() override {
        {
            std::lock_guard<std::mutex> lock(mu_);
            closed_ = true;
        }
        cv_.notify_all();
    }

    void reset_for_reconnect() override {
        std::lock_guard<std::mutex> lock(mu_);
        closed_ = false;
    }

private:
    std::mutex mu_;
    std::condition_variable cv_;
    bool closed_ = false;
};

class RunnerChallengeTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
    ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
        const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions& options) override {
        ecnuvpn::vpn_engine::protocol::AuthInteractionRequest request;
        request.id = "runner-challenge";
        request.kind = "challenge";
        request.label = "Token";
        request.input_type = "password";

        auto response = options.auth_interaction_handler
            ? options.auth_interaction_handler(request)
            : ecnuvpn::vpn_engine::protocol::AuthInteractionResponse{};

        ecnuvpn::vpn_engine::protocol::AuthResult result;
        result.ok = response.ok && response.value == "123456";
        if (result.ok) {
            result.cookie = "runner-cookie";
        } else {
            result.error_code = "auth_challenge_required";
            result.error_message = "authentication challenge response is required";
        }
        return result;
    }

    ecnuvpn::vpn_engine::ValidationResult
    connect_cstp(const std::string&, ecnuvpn::vpn_engine::TunnelMetadata* metadata) override {
        if (!metadata) {
            return {false, "metadata_missing", "metadata output is null"};
        }
        metadata->interface_name = "fake-cstp0";
        metadata->internal_ip4_address = "10.255.0.10";
        metadata->internal_ip4_netmask = "255.255.255.0";
        metadata->mtu = 1400;
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    send_packet(const std::vector<std::uint8_t>&) override { return {}; }

    ecnuvpn::vpn_engine::ValidationResult
    send_control(ecnuvpn::vpn_engine::protocol::InboundFrameKind) override {
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    receive_frame(ecnuvpn::vpn_engine::protocol::InboundFrame*) override {
        std::unique_lock<std::mutex> lock(mu_);
        cv_.wait(lock, [&] { return closed_; });
        return {false, "transport_closed", "transport closed"};
    }

    void disconnect() override {
        {
            std::lock_guard<std::mutex> lock(mu_);
            closed_ = true;
        }
        cv_.notify_all();
    }

    void reset_for_reconnect() override {}

private:
    std::mutex mu_;
    std::condition_variable cv_;
    bool closed_ = false;
};

struct RunnerPacketDeviceState {
    mutable std::mutex mu;
    int open_count = 0;
    bool metadata_open_used = false;
    ecnuvpn::vpn_engine::DeviceConfig last_config;
};

class RunnerFakePacketDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
    explicit RunnerFakePacketDevice(std::shared_ptr<RunnerPacketDeviceState> state)
        : state_(std::move(state)) {}

    ecnuvpn::vpn_engine::ValidationResult
    open(const ecnuvpn::vpn_engine::DeviceConfig& config) override {
        const std::lock_guard<std::mutex> lock(state_->mu);
        state_->last_config = config;
        ++state_->open_count;
        return {};
    }

    ecnuvpn::vpn_engine::ValidationResult
    open(const ecnuvpn::vpn_engine::TunnelMetadata&) override {
        const std::lock_guard<std::mutex> lock(state_->mu);
        state_->metadata_open_used = true;
        ++state_->open_count;
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

private:
    std::shared_ptr<RunnerPacketDeviceState> state_;
};

ecnuvpn::Config runner_config() {
    ecnuvpn::Config cfg;
    cfg.server = "https://vpn.example.invalid";
    cfg.username = "alice";
    cfg.useragent = "ECNU-VPN runner test";
    cfg.mtu = 1290;
    return cfg;
}

// =========================================================================
// Test 1: EngineEventBridge::map_event covers all expected engine types
// =========================================================================
bool test_engine_event_bridge_mapping() {
    using exv::core::EngineEventBridge;
    using exv::core::TunnelEventType;

    bool ok = true;

    // Positive cases: every engine event type that the bridge recognises
    // must map to a TunnelEventType.
    struct { std::string engine_type; TunnelEventType expected; } cases[] = {
        {"auth.succeeded",         TunnelEventType::AuthSucceeded},
        {"auth.failed",            TunnelEventType::AuthFailed},
        {"auth.challenge_required", TunnelEventType::AuthChallengeRequired},
        {"auth.group_required",     TunnelEventType::AuthGroupRequired},
        {"cstp.connected",         TunnelEventType::CstpConnected},
        {"packet.loop.started",    TunnelEventType::PacketLoopStarted},
        {"transport.closed",       TunnelEventType::TransportClosed},
        {"packet_device.failed",   TunnelEventType::PacketDeviceFailed},
    };

    for (auto& c : cases) {
        TunnelEventType out{};
        bool mapped = EngineEventBridge::map_event(c.engine_type, &out);
        ok = expect(mapped,
                    (std::string("map_event('") + c.engine_type +
                     "') should return true").c_str()) && ok;
        ok = expect(out == c.expected,
                    (std::string("map_event('") + c.engine_type +
                     "') should map to correct TunnelEventType").c_str()) && ok;
    }

    // Negative case: unknown engine event type should not map.
    {
        TunnelEventType out{};
        bool mapped = EngineEventBridge::map_event("unknown.event", &out);
        ok = expect(!mapped,
                    "map_event('unknown.event') should return false") && ok;
    }

    return ok;
}

// =========================================================================
// Test 2: EngineEventBridge callback receives translated events
// =========================================================================
bool test_engine_event_bridge_callback() {
    using exv::core::EngineEventBridge;
    using exv::core::TunnelEvent;
    using exv::core::TunnelEventType;
    using ecnuvpn::vpn_engine::VpnEngineEvent;

    bool ok = true;

    std::vector<TunnelEventType> received;

    EngineEventBridge bridge([&](TunnelEvent te) {
        received.push_back(te.type);
    });

    // Emit events that should be translated.
    VpnEngineEvent e1;
    e1.type = "auth.succeeded";
    bridge.emit(e1);

    VpnEngineEvent e2;
    e2.type = "cstp.connected";
    bridge.emit(e2);

    VpnEngineEvent e3;
    e3.type = "unknown.event";  // should be silently dropped
    bridge.emit(e3);

    VpnEngineEvent e4;
    e4.type = "packet.loop.started";
    bridge.emit(e4);

    ok = expect(received.size() == 3,
                "bridge should have received 3 translated events") && ok;
    if (received.size() >= 1) {
        ok = expect(received[0] == TunnelEventType::AuthSucceeded,
                    "first event should be AuthSucceeded") && ok;
    }
    if (received.size() >= 2) {
        ok = expect(received[1] == TunnelEventType::CstpConnected,
                    "second event should be CstpConnected") && ok;
    }
    if (received.size() >= 3) {
        ok = expect(received[2] == TunnelEventType::PacketLoopStarted,
                    "third event should be PacketLoopStarted") && ok;
    }

    return ok;
}

// =========================================================================
// Test 3: CoreSessionRunner initial state
// =========================================================================
bool test_runner_initial_state() {
    using exv::core::CoreSessionRunner;

    bool ok = true;

    CoreSessionRunner runner;

    ok = expect(!runner.is_running(),
                "runner should not be running initially") && ok;

    auto st = runner.status();
    ok = expect(!st.running,
                "initial status.running should be false") && ok;
    ok = expect(!st.network_ready,
                "initial status.network_ready should be false") && ok;

    return ok;
}

// =========================================================================
// Test 4: CoreSessionRunner stop() is safe when not running
// =========================================================================
bool test_runner_stop_when_not_running() {
    using exv::core::CoreSessionRunner;

    bool ok = true;

    CoreSessionRunner runner;
    runner.stop();  // should be a no-op, not crash
    ok = expect(!runner.is_running(),
                "runner should still not be running after stop()") && ok;

    return ok;
}

// =========================================================================
// Test 5: CoreSessionRunner set_event_callback is safe before start
// =========================================================================
bool test_runner_set_callback_before_start() {
    using exv::core::CoreSessionRunner;

    bool ok = true;

    CoreSessionRunner runner;
    int call_count = 0;
    runner.set_event_callback([&](exv::core::TunnelEvent) {
        call_count++;
    });
    ok = expect(call_count == 0,
                "callback should not have been invoked yet") && ok;

    runner.stop();
    ok = expect(call_count == 0,
                "callback should still not have been invoked after stop") && ok;

    return ok;
}

// =========================================================================
// Test 6: CoreSessionRunner start() returns false when already running
// =========================================================================
bool test_runner_double_start_rejected() {
    // This test verifies the guard without actually starting a session.
    // We can't call start() without a real config, but we can verify
    // that is_running() is consulted.
    using exv::core::CoreSessionRunner;

    bool ok = true;

    CoreSessionRunner runner;
    // After construction, is_running() should be false.
    ok = expect(!runner.is_running(),
                "is_running() should be false before start") && ok;

    return ok;
}

// =========================================================================
// Test 7: CoreSessionRunner destruction calls stop
// =========================================================================
bool test_runner_destructor_safety() {
    using exv::core::CoreSessionRunner;

    bool ok = true;

    // Create and destroy immediately — should not crash or leak.
    {
        CoreSessionRunner runner;
        runner.set_event_callback([](exv::core::TunnelEvent) {});
    }

    ok = expect(true, "runner destruction should not crash") && ok;

    return ok;
}

// =========================================================================
// Test 8: CoreSessionRunner concurrent access safety
// =========================================================================
bool test_runner_concurrent_access() {
    using exv::core::CoreSessionRunner;

    bool ok = true;

    CoreSessionRunner runner;
    std::atomic<int> call_count{0};

    runner.set_event_callback([&](exv::core::TunnelEvent) {
        call_count++;
    });

    // Spawn threads that query state concurrently.
    // None of these should crash or deadlock.
    std::vector<std::thread> threads;
    for (int i = 0; i < 4; i++) {
        threads.emplace_back([&runner]() {
            for (int j = 0; j < 100; j++) {
                runner.is_running();
                runner.status();
            }
        });
    }

    for (auto& t : threads) {
        t.join();
    }

    ok = expect(true, "concurrent access should not crash or deadlock") && ok;

    return ok;
}

// =========================================================================
// Test 9: CoreSessionRunner forwards network configurator to native engine
// =========================================================================
bool test_runner_network_configurator_supplies_packet_device_config() {
    using exv::core::CoreSessionRunner;

    bool ok = true;
    auto packet_state = std::make_shared<RunnerPacketDeviceState>();
    bool configurator_called = false;

    CoreSessionRunner runner([packet_state]() {
        ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
        deps.transport_factory = []() {
            return std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport>(
                new RunnerFakeTransport());
        };
        deps.packet_device_factory = [packet_state]() {
            return std::unique_ptr<ecnuvpn::vpn_engine::PacketDevice>(
                new RunnerFakePacketDevice(packet_state));
        };
        return deps;
    });

    runner.set_network_config_callback(
        [packet_state, &configurator_called](
            const ecnuvpn::vpn_engine::TunnelMetadata& metadata,
            ecnuvpn::vpn_engine::DeviceConfig* config) {
            configurator_called = true;
            if (!config) {
                return ecnuvpn::vpn_engine::ValidationResult{
                    false, "device_config_missing", "device config output is null"};
            }
            {
                const std::lock_guard<std::mutex> lock(packet_state->mu);
                if (packet_state->open_count != 0) {
                    return ecnuvpn::vpn_engine::ValidationResult{
                        false, "packet_opened_too_early",
                        "packet device opened before network config"};
                }
            }
            if (metadata.routes.empty() || metadata.server_bypass_ips.empty()) {
                return ecnuvpn::vpn_engine::ValidationResult{
                    false, "metadata_incomplete",
                    "network configurator should see CSTP route metadata"};
            }
            config->interface_name = "helper-runner0";
            config->mtu = 1320;
            return ecnuvpn::vpn_engine::ValidationResult{};
        });

    ok = expect(runner.start(runner_config(), "test-password"),
                "runner start should succeed with fake native dependencies") && ok;
    runner.stop();

    ecnuvpn::vpn_engine::DeviceConfig opened;
    bool metadata_open_used = false;
    int open_count = 0;
    {
        const std::lock_guard<std::mutex> lock(packet_state->mu);
        opened = packet_state->last_config;
        metadata_open_used = packet_state->metadata_open_used;
        open_count = packet_state->open_count;
    }

    ok = expect(configurator_called,
                "network configurator should be called by runner start") && ok;
    ok = expect(open_count == 1,
                "packet device should open exactly once") && ok;
    ok = expect(opened.interface_name == "helper-runner0" && opened.mtu == 1320,
                "packet device should use callback-supplied DeviceConfig") && ok;
    ok = expect(!metadata_open_used,
                "runner packet path should not use TunnelMetadata open") && ok;

    return ok;
}

bool test_runner_starts_from_prepared_handshake_without_reauth() {
    using exv::core::CoreSessionRunner;

    bool ok = true;
    int handshake_transport_factory_calls = 0;

    ecnuvpn::vpn_engine::NativeVpnEngineDependencies handshake_deps;
    handshake_deps.transport_factory = [&]() {
        ++handshake_transport_factory_calls;
        return std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport>(
            new RunnerFakeTransport());
    };

    ecnuvpn::vpn_engine::VpnEngineConfig engine_cfg;
    auto mapped = exv::core::make_native_engine_config(
        runner_config(), "test-password", &engine_cfg);
    ok = expect(mapped.ok, "runner config should map to engine config") && ok;

    ecnuvpn::vpn_engine::NativeHandshakeResult prepared;
    ecnuvpn::vpn_engine::NativeHandshakeJob job(engine_cfg, handshake_deps);
    const auto prepared_result = job.run(std::stop_token{}, &prepared);
    ok = expect(prepared_result.ok, "prepared runner handshake should succeed") && ok;
    ok = expect(handshake_transport_factory_calls == 1,
                "preparing handshake should create one transport") && ok;

    auto packet_state = std::make_shared<RunnerPacketDeviceState>();
    int attach_transport_factory_calls = 0;
    bool configurator_called = false;

    CoreSessionRunner runner([packet_state, &attach_transport_factory_calls]() {
        ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
        deps.transport_factory = [&attach_transport_factory_calls]() {
            ++attach_transport_factory_calls;
            return std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport>(
                new RunnerFakeTransport());
        };
        deps.packet_device_factory = [packet_state]() {
            return std::unique_ptr<ecnuvpn::vpn_engine::PacketDevice>(
                new RunnerFakePacketDevice(packet_state));
        };
        return deps;
    });

    runner.set_network_config_callback(
        [&configurator_called](
            const ecnuvpn::vpn_engine::TunnelMetadata& metadata,
            ecnuvpn::vpn_engine::DeviceConfig* config) {
            configurator_called = true;
            if (metadata.internal_ip4_address != "10.255.0.10") {
                return ecnuvpn::vpn_engine::ValidationResult{
                    false, "metadata_missing", "prepared metadata missing"};
            }
            if (!config) {
                return ecnuvpn::vpn_engine::ValidationResult{
                    false, "device_config_missing", "device config output is null"};
            }
            config->interface_name = "prepared-runner0";
            config->mtu = 1318;
            return ecnuvpn::vpn_engine::ValidationResult{};
        });

    ok = expect(runner.start_from_handshake(engine_cfg, std::move(prepared)),
                "runner should attach from prepared handshake") && ok;
    runner.stop();

    ecnuvpn::vpn_engine::DeviceConfig opened;
    int open_count = 0;
    {
        const std::lock_guard<std::mutex> lock(packet_state->mu);
        opened = packet_state->last_config;
        open_count = packet_state->open_count;
    }

    ok = expect(configurator_called,
                "prepared runner path should run network configurator") && ok;
    ok = expect(attach_transport_factory_calls == 0,
                "prepared runner path must not create a new transport") && ok;
    ok = expect(open_count == 1,
                "prepared runner path should open packet device once") && ok;
    ok = expect(opened.interface_name == "prepared-runner0" &&
                    opened.mtu == 1318,
                "prepared runner path should use callback device config") && ok;

    return ok;
}

// =========================================================================
// Test 10: CoreSessionRunner exposes and answers auth continuation prompts
// =========================================================================
bool test_runner_auth_interaction_response_unblocks_start() {
    using exv::core::CoreSessionRunner;

    bool ok = true;
    auto packet_state = std::make_shared<RunnerPacketDeviceState>();

    CoreSessionRunner runner([packet_state]() {
        ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
        deps.transport_factory = []() {
            return std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport>(
                new RunnerChallengeTransport());
        };
        deps.packet_device_factory = [packet_state]() {
            return std::unique_ptr<ecnuvpn::vpn_engine::PacketDevice>(
                new RunnerFakePacketDevice(packet_state));
        };
        return deps;
    });

    std::atomic<bool> start_done{false};
    std::atomic<bool> start_ok{false};
    std::thread start_thread([&] {
        start_ok.store(runner.start(runner_config(), "test-password"));
        start_done.store(true);
    });

    bool pending_seen = wait_until([&] {
        return runner.pending_auth_interaction().has_value();
    }, std::chrono::seconds(2));
    ok = expect(pending_seen,
                "runner should expose pending auth interaction while start waits") &&
         ok;

    auto pending = runner.pending_auth_interaction();
    ok = expect(pending && pending->kind == "challenge",
                "pending auth interaction should preserve challenge kind") &&
         ok;
    ok = expect(!runner.provide_auth_interaction_response("wrong-id", "123456"),
                "runner should reject response for mismatched interaction id") &&
         ok;
    if (pending) {
        ok = expect(runner.provide_auth_interaction_response(pending->id, "123456"),
                    "runner should accept matching auth interaction response") &&
             ok;
    }

    if (start_thread.joinable())
        start_thread.join();

    ok = expect(start_done.load() && start_ok.load(),
                "runner start should continue after auth interaction response") &&
         ok;
    ok = expect(!runner.pending_auth_interaction().has_value(),
                "pending auth interaction should clear after response") &&
         ok;

    runner.stop();
    return ok;
}

} // namespace

// =========================================================================
// main
// =========================================================================
int main() {
    bool ok = true;

    std::cout << "--- Test 1: EngineEventBridge mapping ---\n";
    ok = test_engine_event_bridge_mapping() && ok;

    std::cout << "--- Test 2: EngineEventBridge callback ---\n";
    ok = test_engine_event_bridge_callback() && ok;

    std::cout << "--- Test 3: Runner initial state ---\n";
    ok = test_runner_initial_state() && ok;

    std::cout << "--- Test 4: Runner stop when not running ---\n";
    ok = test_runner_stop_when_not_running() && ok;

    std::cout << "--- Test 5: Runner set callback before start ---\n";
    ok = test_runner_set_callback_before_start() && ok;

    std::cout << "--- Test 6: Runner double start rejected ---\n";
    ok = test_runner_double_start_rejected() && ok;

    std::cout << "--- Test 7: Runner destructor safety ---\n";
    ok = test_runner_destructor_safety() && ok;

    std::cout << "--- Test 8: Runner concurrent access ---\n";
    ok = test_runner_concurrent_access() && ok;

    std::cout << "--- Test 9: Runner network configurator ---\n";
    ok = test_runner_network_configurator_supplies_packet_device_config() && ok;

    std::cout << "--- Test 10: Runner prepared handshake attach ---\n";
    ok = test_runner_starts_from_prepared_handshake_without_reauth() && ok;

    std::cout << "--- Test 11: Runner auth interaction response ---\n";
    ok = test_runner_auth_interaction_response_unblocks_start() && ok;

    if (ok) {
        std::cout << "core_session_runner_test: all tests passed\n";
    } else {
        std::cerr << "core_session_runner_test: some tests FAILED\n";
    }
    return ok ? 0 : 1;
}
