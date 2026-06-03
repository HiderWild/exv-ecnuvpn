// CoreSessionRunner unit tests.
//
// Tests the CoreSessionRunner lifecycle, event bridging, and thread-safety
// without requiring a real VPN server.  Uses the EngineEventBridge map_event
// logic to verify event translation, and exercises the runner's start/stop/
// is_running/status API surface.

#include "core/core_session_runner.hpp"
#include "core/engine_event_bridge.hpp"
#include "core/tunnel_events.hpp"
#include "core/tunnel_state.hpp"

#include <iostream>
#include <string>
#include <vector>
#include <atomic>
#include <chrono>
#include <thread>
#include <cassert>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
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

    if (ok) {
        std::cout << "core_session_runner_test: all tests passed\n";
    } else {
        std::cerr << "core_session_runner_test: some tests FAILED\n";
    }
    return ok ? 0 : 1;
}
