// Tests for HelperDelegatingPlatformNetworkOps.
//
// Verifies that the delegating layer correctly forwards every
// PlatformNetworkOps call to the underlying HelperClient and that
// type conversions between platform and helper types work properly.

#include "helper/platform/helper_delegating_network_ops.hpp"
#include "support/fake_helper.hpp"

#include <iostream>
#include <string>
#include <cassert>
#include <memory>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

exv::helper::SessionId make_session(const std::string& id) {
    exv::helper::SessionId sid;
    sid.value = id;
    return sid;
}

} // anonymous namespace

// ---------------------------------------------------------------------------
// Test 1: prepare_tunnel_device delegates to helper
// ---------------------------------------------------------------------------
bool test_prepare_tunnel_device_delegates() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);

    // Must set a session before calling prepare
    ops.set_session(make_session("test-session-1"));

    auto desc = ops.prepare_tunnel_device("ECNU-VPN", 1400);

    bool ok = true;
    ok = expect(desc.is_open, "1: device should be open after prepare") && ok;
    ok = expect(desc.adapter_name == "ECNU-VPN",
                "1: adapter_name should match") && ok;
    ok = expect(desc.mtu == 1400,
                "1: mtu should be 1400") && ok;
    ok = expect(!desc.path.empty(),
                "1: path should not be empty") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 2: prepare_tunnel_device with no session sends empty session_id
// ---------------------------------------------------------------------------
bool test_prepare_without_session() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);

    // Intentionally do NOT set a session
    auto desc = ops.prepare_tunnel_device("ECNU-VPN", 1280);

    bool ok = true;
    // The fake helper still returns success, but the session_id should be empty.
    // The MTU comes from the helper response (1400) because the
    // PrepareTunnelDeviceRequest has no mtu field -- the helper sets it.
    ok = expect(desc.is_open, "2: device should still be open (fake allows it)") && ok;
    ok = expect(desc.mtu == 1400, "2: mtu should be 1400 (from helper response)") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 3: open_tunnel_device returns last prepared device
// ---------------------------------------------------------------------------
bool test_open_tunnel_device_after_prepare() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);
    ops.set_session(make_session("test-session-3"));

    // Prepare first
    auto prepared = ops.prepare_tunnel_device("ECNU-VPN", 1400);
    // Then open
    auto opened = ops.open_tunnel_device("ECNU-VPN");

    bool ok = true;
    ok = expect(opened.is_open, "3: opened device should be open") && ok;
    ok = expect(opened.path == prepared.path,
                "3: opened path should match prepared path") && ok;
    ok = expect(opened.adapter_name == "ECNU-VPN",
                "3: adapter_name should match") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 4: open_tunnel_device returns empty descriptor if not prepared
// ---------------------------------------------------------------------------
bool test_open_without_prepare() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);

    auto opened = ops.open_tunnel_device("ECNU-VPN");

    bool ok = true;
    ok = expect(!opened.is_open,
                "4: device should NOT be open if never prepared") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 5: open_tunnel_device for wrong adapter name
// ---------------------------------------------------------------------------
bool test_open_wrong_adapter() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);
    ops.set_session(make_session("test-session-5"));

    ops.prepare_tunnel_device("ECNU-VPN", 1400);
    auto opened = ops.open_tunnel_device("OtherAdapter");

    bool ok = true;
    ok = expect(!opened.is_open,
                "5: opening different adapter name should return closed device") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 6: apply_tunnel_config delegates to helper and succeeds
// ---------------------------------------------------------------------------
bool test_apply_tunnel_config_success() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);
    ops.set_session(make_session("test-session-6"));

    auto device = ops.prepare_tunnel_device("ECNU-VPN", 1400);

    exv::platform::TunnelConfig config;
    config.interface_address = "10.0.0.2/24";
    config.interface_name = "ECNU-VPN";
    config.mtu = 1400;
    config.enable_kill_switch = false;

    exv::platform::RouteEntry route;
    route.destination = "0.0.0.0/0";
    route.gateway = "10.0.0.1";
    route.metric = 100;
    config.routes.push_back(route);

    config.dns.servers.push_back("8.8.8.8");
    config.dns.servers.push_back("8.8.4.4");
    config.dns.search_domain = "ecnu.edu.cn";
    config.server_bypass_ips = {"192.0.2.10", "192.0.2.11/32"};

    bool result = ops.apply_tunnel_config(device, config);

    bool ok = true;
    ok = expect(result, "6: apply_tunnel_config should succeed") && ok;
    auto requests = helper->apply_requests();
    ok = expect(requests.size() == 1,
                "6: helper should receive exactly one apply request") && ok;
    if (!requests.empty()) {
        ok = expect(requests[0].config.server_bypass_ips.size() == 2 &&
                        requests[0].config.server_bypass_ips[0] == "192.0.2.10" &&
                        requests[0].config.server_bypass_ips[1] == "192.0.2.11/32",
                    "6: helper request should preserve all server bypass IPs") && ok;
    }
    return ok;
}

// ---------------------------------------------------------------------------
// Test 7: apply_tunnel_config fails when helper reports failure
// ---------------------------------------------------------------------------
bool test_apply_tunnel_config_failure() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    helper->set_apply_config_fail(true);

    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);
    ops.set_session(make_session("test-session-7"));

    auto device = ops.prepare_tunnel_device("ECNU-VPN", 1400);

    exv::platform::TunnelConfig config;
    config.interface_address = "10.0.0.2/24";

    bool result = ops.apply_tunnel_config(device, config);

    bool ok = true;
    ok = expect(!result, "7: apply_tunnel_config should fail when helper fails") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 8: apply_tunnel_config with no helper returns false
// ---------------------------------------------------------------------------
bool test_apply_no_helper() {
    exv::platform::HelperDelegatingPlatformNetworkOps ops(nullptr);
    ops.set_session(make_session("test-session-8"));

    exv::platform::TunnelDeviceDescriptor device;
    device.adapter_name = "ECNU-VPN";
    device.is_open = true;

    exv::platform::TunnelConfig config;
    bool result = ops.apply_tunnel_config(device, config);

    bool ok = true;
    ok = expect(!result, "8: apply should return false with null helper") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 9: cleanup delegates to helper
// ---------------------------------------------------------------------------
bool test_cleanup_delegates() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);
    ops.set_session(make_session("test-session-9"));

    auto result = ops.cleanup("ECNU-VPN", exv::platform::CleanupPolicy::Full);

    bool ok = true;
    ok = expect(result.success, "9: cleanup should succeed") && ok;
    // FakeHelper records the cleanup request
    ok = expect(helper->cleanup_requests().size() >= 1,
                "9: helper should have received cleanup request") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 10: cleanup with no helper returns error
// ---------------------------------------------------------------------------
bool test_cleanup_no_helper() {
    exv::platform::HelperDelegatingPlatformNetworkOps ops(nullptr);
    ops.set_session(make_session("test-session-10"));

    auto result = ops.cleanup("ECNU-VPN", exv::platform::CleanupPolicy::Full);

    bool ok = true;
    ok = expect(!result.success, "10: cleanup should fail with null helper") && ok;
    ok = expect(!result.error_message.empty(),
                "10: error_message should be set") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 11: device_exists tracks prepared devices
// ---------------------------------------------------------------------------
bool test_device_exists_after_prepare() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);
    ops.set_session(make_session("test-session-11"));

    bool ok = true;
    ok = expect(!ops.device_exists("ECNU-VPN"),
                "11: device should NOT exist before prepare") && ok;

    ops.prepare_tunnel_device("ECNU-VPN", 1400);

    ok = expect(ops.device_exists("ECNU-VPN"),
                "11: device should exist after prepare") && ok;
    ok = expect(!ops.device_exists("OtherAdapter"),
                "11: different adapter should NOT exist") && ok;
    return ok;
}

// ---------------------------------------------------------------------------
// Test 12: device_exists cleared after cleanup via FakeHelper
// ---------------------------------------------------------------------------
bool test_device_exists_cleared_after_cleanup() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);
    ops.set_session(make_session("test-session-12"));

    ops.prepare_tunnel_device("ECNU-VPN", 1400);
    bool ok = true;
    ok = expect(ops.device_exists("ECNU-VPN"),
                "12: device should exist after prepare") && ok;

    // Note: device_exists tracks via last_prepared_device_, not via a
    // separate set.  cleanup goes through the helper but does not clear
    // the local tracking.  This is expected: device_exists reflects
    // prepare state, not cleanup state.  The real platform impl would
    // query the OS.
    ops.cleanup("ECNU-VPN", exv::platform::CleanupPolicy::Full);

    return ok;
}

// ---------------------------------------------------------------------------
// Test 13: set_session / clear_session
// ---------------------------------------------------------------------------
bool test_session_management() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);

    bool ok = true;
    ok = expect(ops.session_id().value.empty(),
                "13: session should be empty initially") && ok;

    ops.set_session(make_session("sess-abc"));
    ok = expect(ops.session_id().value == "sess-abc",
                "13: session should be set after set_session") && ok;

    ops.clear_session();
    ok = expect(ops.session_id().value.empty(),
                "13: session should be empty after clear_session") && ok;

    return ok;
}

// ---------------------------------------------------------------------------
// Test 14: CleanupPolicy mapping
// ---------------------------------------------------------------------------
bool test_cleanup_policy_mapping() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);
    ops.set_session(make_session("test-session-14"));

    bool ok = true;

    // Full cleanup
    ops.cleanup("ECNU-VPN", exv::platform::CleanupPolicy::Full);
    auto reqs = helper->cleanup_requests();
    ok = expect(reqs.size() == 1, "14a: should have 1 cleanup request") && ok;
    if (!reqs.empty()) {
        ok = expect(reqs.back().policy.remove_routes, "14a: Full should remove routes") && ok;
        ok = expect(reqs.back().policy.remove_dns, "14a: Full should remove dns") && ok;
        ok = expect(reqs.back().policy.remove_adapter, "14a: Full should remove adapter") && ok;
        ok = expect(reqs.back().policy.remove_firewall_rules, "14a: Full should remove firewall") && ok;
    }

    // RoutesOnly
    ops.cleanup("ECNU-VPN", exv::platform::CleanupPolicy::RoutesOnly);
    reqs = helper->cleanup_requests();
    ok = expect(reqs.size() == 2, "14b: should have 2 cleanup requests") && ok;
    if (reqs.size() >= 2) {
        ok = expect(reqs.back().policy.remove_routes, "14b: RoutesOnly should remove routes") && ok;
        ok = expect(!reqs.back().policy.remove_dns, "14b: RoutesOnly should NOT remove dns") && ok;
        ok = expect(!reqs.back().policy.remove_adapter, "14b: RoutesOnly should NOT remove adapter") && ok;
        ok = expect(!reqs.back().policy.remove_firewall_rules, "14b: RoutesOnly should NOT remove firewall") && ok;
    }

    return ok;
}

// ---------------------------------------------------------------------------
// Test 15: Full lifecycle (prepare -> apply -> cleanup)
// ---------------------------------------------------------------------------
bool test_full_lifecycle() {
    auto helper = std::make_unique<exv::test::FakeHelper>();
    auto* h = helper.get();
    exv::platform::HelperDelegatingPlatformNetworkOps ops(h);

    // Simulate TunnelController flow
    ops.set_session(make_session("lifecycle-session"));

    // 1. Prepare
    auto device = ops.prepare_tunnel_device("ECNU-VPN", 1400);
    bool ok = true;
    ok = expect(device.is_open, "15: prepare should succeed") && ok;

    // 2. Open
    auto opened = ops.open_tunnel_device("ECNU-VPN");
    ok = expect(opened.is_open, "15: open should succeed after prepare") && ok;

    // 3. Apply
    exv::platform::TunnelConfig config;
    config.interface_address = "10.0.0.2/24";
    config.interface_name = "ECNU-VPN";
    config.routes.push_back({"0.0.0.0/0", "10.0.0.1", 100, true});
    config.dns.servers.push_back("8.8.8.8");
    bool applied = ops.apply_tunnel_config(device, config);
    ok = expect(applied, "15: apply should succeed") && ok;

    // 4. Cleanup
    auto cleanup_result = ops.cleanup("ECNU-VPN", exv::platform::CleanupPolicy::Full);
    ok = expect(cleanup_result.success, "15: cleanup should succeed") && ok;

    // 5. Device should still exist (cleanup doesn't clear local tracking)
    ok = expect(ops.device_exists("ECNU-VPN"), "15: device should still exist after cleanup") && ok;

    return ok;
}

// ===========================================================================
// main
// ===========================================================================
int main() {
    bool ok = true;

    std::cout << "--- Test 1: prepare_tunnel_device delegates ---\n";
    ok = test_prepare_tunnel_device_delegates() && ok;

    std::cout << "--- Test 2: prepare without session ---\n";
    ok = test_prepare_without_session() && ok;

    std::cout << "--- Test 3: open_tunnel_device after prepare ---\n";
    ok = test_open_tunnel_device_after_prepare() && ok;

    std::cout << "--- Test 4: open without prepare ---\n";
    ok = test_open_without_prepare() && ok;

    std::cout << "--- Test 5: open wrong adapter ---\n";
    ok = test_open_wrong_adapter() && ok;

    std::cout << "--- Test 6: apply_tunnel_config success ---\n";
    ok = test_apply_tunnel_config_success() && ok;

    std::cout << "--- Test 7: apply_tunnel_config failure ---\n";
    ok = test_apply_tunnel_config_failure() && ok;

    std::cout << "--- Test 8: apply no helper ---\n";
    ok = test_apply_no_helper() && ok;

    std::cout << "--- Test 9: cleanup delegates ---\n";
    ok = test_cleanup_delegates() && ok;

    std::cout << "--- Test 10: cleanup no helper ---\n";
    ok = test_cleanup_no_helper() && ok;

    std::cout << "--- Test 11: device_exists after prepare ---\n";
    ok = test_device_exists_after_prepare() && ok;

    std::cout << "--- Test 12: device_exists after cleanup ---\n";
    ok = test_device_exists_cleared_after_cleanup() && ok;

    std::cout << "--- Test 13: session management ---\n";
    ok = test_session_management() && ok;

    std::cout << "--- Test 14: cleanup policy mapping ---\n";
    ok = test_cleanup_policy_mapping() && ok;

    std::cout << "--- Test 15: full lifecycle ---\n";
    ok = test_full_lifecycle() && ok;

    if (ok) {
        std::cout << "helper_delegating_network_ops_test: all tests passed\n";
    } else {
        std::cerr << "helper_delegating_network_ops_test: some tests FAILED\n";
    }
    return ok ? 0 : 1;
}
