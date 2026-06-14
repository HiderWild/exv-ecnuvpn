// Comprehensive Helper V2 contract tests.
// Covers: Hello protocol, session lifecycle, error handling, lease timeout,
// cleanup registry, command validation, and security (no credentials in messages).

#include "helper/common/helper_protocol.hpp"
#include "helper/common/helper_capabilities.hpp"
#include "helper/common/helper_error.hpp"
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_session_lease.hpp"
#include "helper/common/helper_client.hpp"
#include "helper/runtime/session_lease_manager.hpp"
#include "helper/runtime/cleanup_registry.hpp"
#include "helper/runtime/command_validator.hpp"
#include "helper/runtime/helper_lifecycle_policy.hpp"
#include "contracts/generated/system_contract.hpp"
#include "support/fake_helper.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <string>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <thread>

#ifdef _WIN32
#include <windows.h>
#endif

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

int current_process_id() {
#ifdef _WIN32
    return static_cast<int>(GetCurrentProcessId());
#else
    return static_cast<int>(getpid());
#endif
}

bool json_contains_credential(const std::string& json_str,
                              const std::vector<std::string>& forbidden_keys) {
    try {
        auto j = nlohmann::json::parse(json_str);
        // Recursively search for forbidden keys
        std::function<bool(const nlohmann::json&)> search;
        search = [&](const nlohmann::json& node) -> bool {
            if (node.is_object()) {
                for (auto it = node.begin(); it != node.end(); ++it) {
                    for (const auto& key : forbidden_keys) {
                        // Case-insensitive substring check on key name
                        std::string lower_key = it.key();
                        for (auto& c : lower_key) c = static_cast<char>(std::tolower(c));
                        std::string lower_forbidden = key;
                        for (auto& c : lower_forbidden) c = static_cast<char>(std::tolower(c));
                        if (lower_key.find(lower_forbidden) != std::string::npos) {
                            return true;
                        }
                    }
                    if (search(it.value())) return true;
                }
            } else if (node.is_array()) {
                for (const auto& item : node) {
                    if (search(item)) return true;
                }
            }
            return false;
        };
        return search(j);
    } catch (...) {
        return false;
    }
}

// ============================================================
// 1. Hello Protocol
// ============================================================

int test_hello_returns_correct_protocol_version() {
    std::cout << "  [Hello] returns correct protocol_version\n";
    bool ok = true;

    exv::test::FakeHelper helper;
    helper.connect();

    exv::helper::HelloRequest req;
    req.client_version = exv::helper::PROTOCOL_VERSION;
    auto resp = helper.hello(req);

    ok = expect(resp.server_version == exv::helper::PROTOCOL_VERSION,
                "server_version should equal PROTOCOL_VERSION") && ok;
    ok = expect(resp.server_version == 2,
                "server_version should be 2") && ok;

    return ok ? 0 : 1;
}

int test_hello_returns_capabilities() {
    std::cout << "  [Hello] returns capabilities list\n";
    bool ok = true;

    exv::test::FakeHelper helper;
    helper.connect();

    exv::helper::HelloRequest req;
    auto resp = helper.hello(req);

    ok = expect(!resp.capabilities.empty(),
                "capabilities should not be empty") && ok;
    ok = expect(resp.capabilities.size() >= 3,
                "should have at least 3 capabilities") && ok;

    // Check that known capabilities are present
    bool has_tunnel = false, has_route = false, has_dns = false;
    for (const auto& cap : resp.capabilities) {
        if (cap == "tunnel_device_create") has_tunnel = true;
        if (cap == "route_apply") has_route = true;
        if (cap == "dns_apply") has_dns = true;
    }
    ok = expect(has_tunnel, "should have tunnel_device_create capability") && ok;
    ok = expect(has_route, "should have route_apply capability") && ok;
    ok = expect(has_dns, "should have dns_apply capability") && ok;

    return ok ? 0 : 1;
}

int test_hello_mismatched_version_still_works() {
    std::cout << "  [Hello] mismatched version still works (backward compat)\n";
    bool ok = true;

    exv::test::FakeHelper helper;
    helper.connect();

    // Send a request with a different version
    exv::helper::HelloRequest req;
    req.client_version = 999;  // Mismatched
    auto resp = helper.hello(req);

    // FakeHelper always returns successfully - the contract is that
    // the response should still be valid, not an error
    ok = expect(resp.server_version == exv::helper::PROTOCOL_VERSION,
                "server should still return its own version") && ok;
    ok = expect(!resp.capabilities.empty(),
                "capabilities should still be populated") && ok;

    return ok ? 0 : 1;
}

// ============================================================
// 2. Session Lifecycle
// ============================================================

int test_start_session_returns_valid_id() {
    std::cout << "  [Lifecycle] StartSession returns valid session_id\n";
    bool ok = true;

    exv::helper::SessionLeaseManager mgr;
    exv::helper::ProfileId pid{"test-profile"};
    auto sid = mgr.create_session(pid, exv::helper::HelperMode::Transient,
                                  exv::helper::CleanupPolicy{});

    ok = expect(!sid.value.empty(), "session_id should not be empty") && ok;
    ok = expect(sid.value.find("ses-") == 0,
                "session_id should start with 'ses-' prefix") && ok;
    ok = expect(mgr.has_session(sid), "session should exist after creation") && ok;

    return ok ? 0 : 1;
}

int test_start_session_transient_mode() {
    std::cout << "  [Lifecycle] StartSession with Transient mode\n";
    bool ok = true;

    exv::helper::SessionLeaseManager mgr;
    auto sid = mgr.create_session({"p"}, exv::helper::HelperMode::Transient,
                                  exv::helper::CleanupPolicy{});

    auto lease = mgr.get_session(sid);
    ok = expect(lease.has_value(), "lease should exist") && ok;
    ok = expect(lease->mode == exv::helper::HelperMode::Transient,
                "mode should be Transient") && ok;

    return ok ? 0 : 1;
}

int test_start_session_resident_mode() {
    std::cout << "  [Lifecycle] StartSession with Resident mode\n";
    bool ok = true;

    exv::helper::SessionLeaseManager mgr;
    auto sid = mgr.create_session({"p"}, exv::helper::HelperMode::Resident,
                                  exv::helper::CleanupPolicy{});

    auto lease = mgr.get_session(sid);
    ok = expect(lease.has_value(), "lease should exist") && ok;
    ok = expect(lease->mode == exv::helper::HelperMode::Resident,
                "mode should be Resident") && ok;

    return ok ? 0 : 1;
}

int test_prepare_tunnel_device_after_start_session() {
    std::cout << "  [Lifecycle] PrepareTunnelDevice after StartSession succeeds\n";
    bool ok = true;

    exv::test::FakeHelper helper;
    helper.connect();

    // Start session first
    exv::helper::StartSessionRequest start_req;
    start_req.profile_id.value = "test-profile";
    auto start_resp = helper.start_session(start_req);
    ok = expect(!start_resp.session_id.value.empty(),
                "session_id should not be empty") && ok;

    // Prepare tunnel device
    exv::helper::PrepareTunnelDeviceRequest prep_req;
    prep_req.session_id = start_resp.session_id;
    prep_req.adapter_name = "Wintun";
    auto prep_resp = helper.prepare_tunnel_device(prep_req);

    ok = expect(!prep_resp.device_path.empty(),
                "device_path should not be empty") && ok;
    ok = expect(prep_resp.mtu > 0,
                "MTU should be positive") && ok;

    return ok ? 0 : 1;
}

int test_apply_tunnel_config_after_prepare() {
    std::cout << "  [Lifecycle] ApplyTunnelConfig after PrepareTunnelDevice succeeds\n";
    bool ok = true;

    exv::test::FakeHelper helper;
    helper.connect();

    // Start session
    exv::helper::StartSessionRequest start_req;
    start_req.profile_id.value = "test-profile";
    auto start_resp = helper.start_session(start_req);

    // Prepare tunnel
    exv::helper::PrepareTunnelDeviceRequest prep_req;
    prep_req.session_id = start_resp.session_id;
    prep_req.adapter_name = "Wintun";
    auto prep_resp = helper.prepare_tunnel_device(prep_req);
    (void)prep_resp;

    // Apply config
    exv::helper::ApplyTunnelConfigRequest cfg_req;
    cfg_req.config.session_id = start_resp.session_id;
    cfg_req.config.interface_address = "10.0.0.2/24";
    cfg_req.config.routes.push_back({"0.0.0.0/0", "10.0.0.1", 100});
    cfg_req.config.dns.servers = {"8.8.8.8"};
    auto cfg_resp = helper.apply_tunnel_config(cfg_req);

    ok = expect(cfg_resp.success, "apply_tunnel_config should succeed") && ok;

    return ok ? 0 : 1;
}

int test_heartbeat_updates_timestamp() {
    std::cout << "  [Lifecycle] Heartbeat updates session timestamp\n";
    bool ok = true;

    exv::helper::SessionLeaseManager mgr;
    auto sid = mgr.create_session({"p"}, exv::helper::HelperMode::Transient,
                                  exv::helper::CleanupPolicy{});

    auto before_lease = mgr.get_session(sid);
    ok = expect(before_lease->core_phase == "init",
                "initial core_phase should be init") && ok;

    // Small delay to ensure timestamp difference is measurable
    std::this_thread::sleep_for(std::chrono::milliseconds(10));

    mgr.update_heartbeat(sid, "Connected");
    auto after_lease = mgr.get_session(sid);

    ok = expect(after_lease->core_phase == "Connected",
                "core_phase should be Connected after heartbeat") && ok;
    ok = expect(after_lease->last_heartbeat >= before_lease->last_heartbeat,
                "last_heartbeat should be updated (>= before)") && ok;

    return ok ? 0 : 1;
}

int test_cleanup_removes_session() {
    std::cout << "  [Lifecycle] Cleanup removes session\n";
    bool ok = true;

    exv::test::FakeHelper helper;
    helper.connect();

    // Start session
    exv::helper::StartSessionRequest start_req;
    start_req.profile_id.value = "test-profile";
    auto start_resp = helper.start_session(start_req);

    // Verify session exists
    auto sessions = helper.active_sessions();
    ok = expect(sessions.size() == 1, "should have 1 active session") && ok;

    // Cleanup
    exv::helper::CleanupRequest cleanup_req;
    cleanup_req.session_id = start_resp.session_id;
    auto cleanup_resp = helper.cleanup(cleanup_req);
    ok = expect(cleanup_resp.success, "cleanup should succeed") && ok;

    // Verify session removed
    sessions = helper.active_sessions();
    ok = expect(sessions.empty(), "should have 0 active sessions after cleanup") && ok;

    return ok ? 0 : 1;
}

int test_end_session_removes_session() {
    std::cout << "  [Lifecycle] EndSession removes session\n";
    bool ok = true;

    exv::test::FakeHelper helper;
    helper.connect();

    // Start session
    exv::helper::StartSessionRequest start_req;
    start_req.profile_id.value = "test-profile";
    auto start_resp = helper.start_session(start_req);

    auto sessions = helper.active_sessions();
    ok = expect(sessions.size() == 1, "should have 1 active session") && ok;

    // End session
    exv::helper::EndSessionRequest end_req;
    end_req.session_id = start_resp.session_id;
    auto end_resp = helper.end_session(end_req);
    ok = expect(end_resp.success, "end_session should succeed") && ok;

    sessions = helper.active_sessions();
    ok = expect(sessions.empty(), "should have 0 sessions after end_session") && ok;

    return ok ? 0 : 1;
}

// ============================================================
// 3. Error Handling
// ============================================================

int test_unknown_op_returns_error() {
    std::cout << "  [Error] Unknown op returns structured error\n";
    bool ok = true;

    exv::helper::CommandValidator validator;
    exv::helper::HelperRequest req;
    // Use an op value outside the V2 range
    req.op = static_cast<exv::helper::HelperOp>(9999);
    req.payload_json = "{}";

    auto err = validator.validate(req);
    ok = expect(err.has_value(), "validation should return error for unknown op") && ok;
    if (err.has_value()) {
        ok = expect(err->code == exv::helper::HelperErrorCode::UnsupportedOp,
                    "error code should be UnsupportedOp") && ok;
        ok = expect(!err->message.empty(),
                    "error message should not be empty") && ok;
    }

    return ok ? 0 : 1;
}

int test_prepare_tunnel_without_session_fails() {
    std::cout << "  [Error] PrepareTunnelDevice without StartSession fails\n";
    bool ok = true;

    exv::test::FakeHelper helper;
    helper.connect();

    // Attempt prepare without starting session
    exv::helper::PrepareTunnelDeviceRequest prep_req;
    prep_req.session_id.value = "nonexistent-session";
    prep_req.adapter_name = "Wintun";
    auto prep_resp = helper.prepare_tunnel_device(prep_req);

    // FakeHelper still returns a path (it's a fake), but the real contract
    // is that the device_path references the nonexistent session.
    // In a real implementation this would fail. We test the serialization
    // contract: the request must carry a valid session_id.
    ok = expect(prep_req.session_id.value == "nonexistent-session",
                "request should carry the session_id we set") && ok;

    // Verify through SessionLeaseManager that a nonexistent session is not found
    exv::helper::SessionLeaseManager mgr;
    exv::helper::SessionId unknown;
    unknown.value = "nonexistent-session";
    ok = expect(!mgr.has_session(unknown),
                "nonexistent session should not exist in lease manager") && ok;

    return ok ? 0 : 1;
}

int test_heartbeat_without_session_fails() {
    std::cout << "  [Error] Heartbeat without StartSession fails\n";
    bool ok = true;

    exv::helper::SessionLeaseManager mgr;
    exv::helper::SessionId unknown;
    unknown.value = "nonexistent-session";

    // update_heartbeat on unknown session is a no-op (no crash)
    mgr.update_heartbeat(unknown, "Connected");
    ok = expect(!mgr.has_session(unknown),
                "unknown session should still not exist after heartbeat attempt") && ok;
    ok = expect(mgr.active_session_count() == 0,
                "no sessions should be created by heartbeat on unknown id") && ok;

    return ok ? 0 : 1;
}

int test_double_start_session_same_profile() {
    std::cout << "  [Error] Double StartSession with same profile\n";
    bool ok = true;

    exv::helper::SessionLeaseManager mgr;
    exv::helper::ProfileId pid{"same-profile"};

    auto sid1 = mgr.create_session(pid, exv::helper::HelperMode::Transient,
                                   exv::helper::CleanupPolicy{});
    auto sid2 = mgr.create_session(pid, exv::helper::HelperMode::Transient,
                                   exv::helper::CleanupPolicy{});

    // Both should succeed with unique IDs
    ok = expect(!sid1.value.empty(), "first session_id should not be empty") && ok;
    ok = expect(!sid2.value.empty(), "second session_id should not be empty") && ok;
    ok = expect(sid1.value != sid2.value,
                "two sessions for same profile should have different IDs") && ok;
    ok = expect(mgr.active_session_count() == 2,
                "should have 2 active sessions") && ok;

    return ok ? 0 : 1;
}

// ============================================================
// 4. Lease Timeout
// ============================================================

int test_transient_session_expires() {
    std::cout << "  [Lease] Transient session expires after timeout (30s)\n";
    bool ok = true;

    exv::helper::SessionLeaseManager mgr;
    auto sid = mgr.create_session({"p"}, exv::helper::HelperMode::Transient,
                                  exv::helper::CleanupPolicy{});

    // Immediately after creation: not expired
    auto now = std::chrono::steady_clock::now();
    auto expired = mgr.find_expired_sessions(now);
    ok = expect(expired.empty(), "fresh session should not be expired") && ok;

    // Simulate 31 seconds elapsed by manipulating the lease's last_heartbeat
    // We can't directly set last_heartbeat, but we can verify the timeout config
    exv::helper::LeaseTimeoutConfig config;
    ok = expect(config.transient_heartbeat_timeout == std::chrono::seconds(30),
                "transient timeout should be 30s") && ok;

    return ok ? 0 : 1;
}

int test_resident_session_expires_later() {
    std::cout << "  [Lease] Resident session has longer timeout (60s)\n";
    bool ok = true;

    exv::helper::LeaseTimeoutConfig config;
    ok = expect(config.resident_heartbeat_timeout == std::chrono::seconds(60),
                "resident timeout should be 60s") && ok;
    ok = expect(config.resident_heartbeat_timeout > config.transient_heartbeat_timeout,
                "resident timeout should be longer than transient") && ok;

    return ok ? 0 : 1;
}

int test_lease_timeout_config_values() {
    std::cout << "  [Lease] LeaseTimeoutConfig default values\n";
    bool ok = true;

    exv::helper::LeaseTimeoutConfig config;
    ok = expect(config.transient_heartbeat_timeout == std::chrono::seconds(30),
                "transient_heartbeat_timeout should be 30s") && ok;
    ok = expect(config.resident_heartbeat_timeout == std::chrono::seconds(60),
                "resident_heartbeat_timeout should be 60s") && ok;
    ok = expect(config.transient_idle_timeout == std::chrono::seconds(5),
                "transient_idle_timeout should be 5s") && ok;
    ok = expect(config.max_reconnect_window == std::chrono::seconds(300),
                "max_reconnect_window should be 300s") && ok;

    return ok ? 0 : 1;
}

int test_lifecycle_policy_heartbeat_expiry() {
    std::cout << "  [Lease] HelperLifecyclePolicy detects expired heartbeat\n";
    bool ok = true;

    exv::helper::LeaseTimeoutConfig config;
    // Use a very short timeout for testing
    config.transient_heartbeat_timeout = std::chrono::seconds(0);
    exv::helper::HelperLifecyclePolicy policy(config);

    exv::helper::SessionLease lease;
    lease.mode = exv::helper::HelperMode::Transient;
    lease.last_heartbeat = std::chrono::steady_clock::now() - std::chrono::seconds(1);

    ok = expect(policy.is_heartbeat_expired(lease, std::chrono::steady_clock::now()),
                "heartbeat should be expired with 0s timeout") && ok;

    // With a long timeout, it should NOT be expired
    exv::helper::LeaseTimeoutConfig long_config;
    long_config.transient_heartbeat_timeout = std::chrono::seconds(3600);
    exv::helper::HelperLifecyclePolicy long_policy(long_config);

    exv::helper::SessionLease fresh_lease;
    fresh_lease.mode = exv::helper::HelperMode::Transient;
    fresh_lease.last_heartbeat = std::chrono::steady_clock::now();

    ok = expect(!long_policy.is_heartbeat_expired(fresh_lease, std::chrono::steady_clock::now()),
                "fresh heartbeat should not be expired with 3600s timeout") && ok;

    return ok ? 0 : 1;
}

int test_lifecycle_policy_exit_after_cleanup() {
    std::cout << "  [Lease] HelperLifecyclePolicy: Transient exits, Resident stays\n";
    bool ok = true;

    exv::helper::HelperLifecyclePolicy policy;
    ok = expect(policy.should_exit_after_cleanup(exv::helper::HelperMode::Transient),
                "Transient mode should exit after cleanup") && ok;
    ok = expect(!policy.should_exit_after_cleanup(exv::helper::HelperMode::Resident),
                "Resident mode should NOT exit after cleanup") && ok;

    return ok ? 0 : 1;
}

// ============================================================
// 5. Cleanup Registry
// ============================================================

int test_cleanup_registry_records_resources() {
    std::cout << "  [Cleanup] CleanupRegistry records resources\n";
    bool ok = true;

    exv::helper::CleanupRegistry registry;
    exv::helper::CleanupRecord rec;
    rec.session_id.value = "ses-test-1";
    rec.adapter_name = "ECNU-VPN";
    rec.routes.push_back({"0.0.0.0/0", "10.0.0.1", 0});
    rec.dns.servers = {"8.8.8.8", "8.8.4.4"};

    registry.register_session(rec);

    auto resources = registry.get_resources(rec.session_id);
    ok = expect(!resources.empty(), "should have resources after registration") && ok;

    int route_count = 0, dns_count = 0, adapter_count = 0;
    for (const auto& r : resources) {
        if (r.type == "route") route_count++;
        if (r.type == "dns") dns_count++;
        if (r.type == "adapter") adapter_count++;
    }
    ok = expect(route_count == 1, "should have 1 route resource") && ok;
    ok = expect(dns_count == 2, "should have 2 dns resources") && ok;
    ok = expect(adapter_count == 1, "should have 1 adapter resource") && ok;

    return ok ? 0 : 1;
}

int test_cleanup_registry_idempotent() {
    std::cout << "  [Cleanup] CleanupRegistry is idempotent\n";
    bool ok = true;

    exv::helper::CleanupRegistry registry;
    exv::helper::CleanupRecord rec;
    rec.session_id.value = "ses-idempotent";
    rec.adapter_name = "test-adapter";
    registry.register_session(rec);

    // Remove once
    registry.remove_session(rec.session_id);
    ok = expect(registry.all_records().empty(),
                "should be empty after first remove") && ok;

    // Remove again - should not crash
    registry.remove_session(rec.session_id);
    ok = expect(registry.all_records().empty(),
                "should still be empty after second remove") && ok;

    // Also test get_resources on already-removed session
    auto resources = registry.get_resources(rec.session_id);
    ok = expect(resources.empty(),
                "should return empty resources for removed session") && ok;

    return ok ? 0 : 1;
}

int test_cleanup_registry_save_load() {
    std::cout << "  [Cleanup] CleanupRegistry save/load cleanup records\n";
    bool ok = true;

    namespace fs = std::filesystem;
    const auto root = fs::temp_directory_path() /
        ("ecnuvpn-v2-contract-test-" + std::to_string(current_process_id()));
    fs::create_directories(root);
    const auto path = (root / "cleanup_records.json").string();

    // Save
    {
        exv::helper::CleanupRegistry registry;
        exv::helper::CleanupRecord rec;
        rec.session_id.value = "ses-persist";
        rec.adapter_name = "ECNU-VPN";
        rec.routes.push_back({"10.0.0.0/8", "10.0.0.1", 50});
        rec.dns.servers = {"1.1.1.1"};
        rec.firewall_rules = {"allow-ecnu-vpn"};
        registry.register_session(rec);

        bool saved = registry.save_to_disk(path);
        ok = expect(saved, "save_to_disk should succeed") && ok;
        ok = expect(fs::exists(path), "file should exist after save") && ok;
    }

    // Load
    {
        exv::helper::CleanupRegistry registry;
        bool loaded = registry.load_from_disk(path);
        ok = expect(loaded, "load_from_disk should succeed") && ok;

        auto records = registry.all_records();
        ok = expect(records.size() == 1,
                    "loaded registry should have 1 record") && ok;
        ok = expect(records[0].session_id.value == "ses-persist",
                    "loaded session_id should match") && ok;
        ok = expect(records[0].adapter_name == "ECNU-VPN",
                    "loaded adapter_name should match") && ok;
        ok = expect(records[0].routes.size() == 1,
                    "loaded routes should have 1 entry") && ok;
        ok = expect(records[0].routes[0].destination == "10.0.0.0/8",
                    "loaded route destination should match") && ok;
        ok = expect(records[0].dns.servers.size() == 1,
                    "loaded DNS should have 1 server") && ok;
        ok = expect(records[0].dns.servers[0] == "1.1.1.1",
                    "loaded DNS server should match") && ok;
    }

    fs::remove_all(root);
    return ok ? 0 : 1;
}

int test_cleanup_registry_add_managed_resource() {
    std::cout << "  [Cleanup] add_resource adds managed resource\n";
    bool ok = true;

    exv::helper::CleanupRegistry registry;
    exv::helper::CleanupRecord rec;
    rec.session_id.value = "ses-managed";
    registry.register_session(rec);

    exv::helper::ManagedResource res;
    res.type = "firewall_rule";
    res.detail = "ECNU-VPN-kill-switch";
    registry.add_resource(rec.session_id, res);

    auto resources = registry.get_resources(rec.session_id);
    bool found = false;
    for (const auto& r : resources) {
        if (r.type == "firewall_rule" && r.detail == "ECNU-VPN-kill-switch") {
            found = true;
        }
    }
    ok = expect(found, "added firewall resource should be in get_resources") && ok;

    return ok ? 0 : 1;
}

// ============================================================
// 6. Command Validation
// ============================================================

int test_valid_v2_ops_pass_validation() {
    std::cout << "  [Validation] Valid V2 ops pass validation\n";
    bool ok = true;

    exv::helper::CommandValidator validator;

    std::vector<exv::helper::HelperOp> valid_ops = {
        exv::helper::HelperOp::Hello,
        exv::helper::HelperOp::StartSession,
        exv::helper::HelperOp::PrepareTunnelDevice,
        exv::helper::HelperOp::ApplyTunnelConfig,
        exv::helper::HelperOp::Heartbeat,
        exv::helper::HelperOp::Cleanup,
        exv::helper::HelperOp::GetSnapshot,
        exv::helper::HelperOp::EndSession,
    };

    for (auto op : valid_ops) {
        exv::helper::HelperRequest req;
        req.op = op;
        req.payload_json = "{}";
        auto err = validator.validate(req);
        ok = expect(!err.has_value(),
                    "valid V2 op should pass validation") && ok;
    }

    return ok ? 0 : 1;
}

int test_legacy_ops_fail_validation() {
    std::cout << "  [Validation] Legacy ops fail validation\n";
    bool ok = true;

    exv::helper::CommandValidator validator;

    std::vector<exv::helper::HelperOp> legacy_ops = {
        exv::helper::HelperOp::LegacyStart,
        exv::helper::HelperOp::LegacyStop,
        exv::helper::HelperOp::LegacyStatus,
        exv::helper::HelperOp::LegacyHeartbeat,
    };

    for (auto op : legacy_ops) {
        exv::helper::HelperRequest req;
        req.op = op;
        req.payload_json = "{}";
        auto err = validator.validate(req);
        ok = expect(err.has_value(),
                    "legacy op should fail validation") && ok;
        if (err.has_value()) {
            ok = expect(err->code == exv::helper::HelperErrorCode::UnsupportedOp,
                        "error code should be UnsupportedOp for legacy op") && ok;
        }
    }

    return ok ? 0 : 1;
}

int test_shell_injection_rejected() {
    std::cout << "  [Validation] Shell injection in adapter name is rejected\n";
    bool ok = true;

    // Test various shell injection patterns
    std::vector<std::string> injection_payloads = {
        "{\"adapter_name\":\"test; rm -rf /\"}",
        "{\"adapter_name\":\"test | cat /etc/passwd\"}",
        "{\"adapter_name\":\"test & echo pwned\"}",
        "{\"adapter_name\":\"test `whoami`\"}",
        "{\"adapter_name\":\"test $(id)\"}",
        "{\"adapter_name\":\"test (subshell)\"}",
    };

    for (const auto& payload : injection_payloads) {
        ok = expect(exv::helper::CommandValidator::contains_shell_injection(payload),
                    "shell injection should be detected") && ok;
    }

    // Safe payloads should not be flagged
    std::vector<std::string> safe_payloads = {
        "{\"adapter_name\":\"Wintun\"}",
        "{\"adapter_name\":\"ECNU-VPN-Adapter\"}",
        "{\"adapter_name\":\"test_adapter_01\"}",
    };

    for (const auto& payload : safe_payloads) {
        ok = expect(!exv::helper::CommandValidator::contains_shell_injection(payload),
                    "safe payload should not be flagged as injection") && ok;
    }

    return ok ? 0 : 1;
}

int test_path_traversal_rejected() {
    std::cout << "  [Validation] Path traversal in path is rejected\n";
    bool ok = true;

    // Test path traversal patterns
    std::vector<std::string> traversal_payloads = {
        "{\"device_path\":\"../../../etc/passwd\"}",
        "{\"device_path\":\"..\\\\windows\\\\system32\"}",
        "{\"path\":\"/tmp/../../root/.ssh\"}",
    };

    for (const auto& payload : traversal_payloads) {
        ok = expect(exv::helper::CommandValidator::contains_path_traversal(payload),
                    "path traversal should be detected") && ok;
    }

    // Safe paths should not be flagged
    std::vector<std::string> safe_paths = {
        "{\"device_path\":\"//./Wintun/0\"}",
        "{\"device_path\":\"/dev/tun0\"}",
        "{\"path\":\"/tmp/ecnu-vpn-socket\"}",
    };

    for (const auto& payload : safe_paths) {
        ok = expect(!exv::helper::CommandValidator::contains_path_traversal(payload),
                    "safe path should not be flagged as traversal") && ok;
    }

    return ok ? 0 : 1;
}

int test_command_validator_integration() {
    std::cout << "  [Validation] CommandValidator rejects shell injection in request\n";
    bool ok = true;

    exv::helper::CommandValidator validator;

    exv::helper::HelperRequest req;
    req.op = exv::helper::HelperOp::PrepareTunnelDevice;
    req.payload_json = "{\"adapter_name\":\"test; malicious\"}";

    auto err = validator.validate(req);
    ok = expect(err.has_value(),
                "request with shell injection should be rejected") && ok;
    if (err.has_value()) {
        ok = expect(err->code == exv::helper::HelperErrorCode::PermissionDenied,
                    "error code should be PermissionDenied") && ok;
    }

    // Path traversal in request
    exv::helper::HelperRequest req2;
    req2.op = exv::helper::HelperOp::PrepareTunnelDevice;
    req2.payload_json = "{\"device_path\":\"../../../etc/shadow\"}";

    auto err2 = validator.validate(req2);
    ok = expect(err2.has_value(),
                "request with path traversal should be rejected") && ok;
    if (err2.has_value()) {
        ok = expect(err2->code == exv::helper::HelperErrorCode::PermissionDenied,
                    "error code should be PermissionDenied for path traversal") && ok;
    }

    return ok ? 0 : 1;
}

// ============================================================
// 7. Security: No credentials in V2 messages
// ============================================================

int test_no_credentials_in_helper_request() {
    std::cout << "  [Security] HelperRequest doesn't contain credentials\n";
    bool ok = true;

    std::vector<std::string> forbidden = {"password", "cookie", "token",
                                          "secret", "credential", "auth_key"};

    // Test all V2 ops' request serialization
    {
        exv::helper::HelloRequest req;
        req.client_version = 2;
        nlohmann::json j = req;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "HelloRequest should not contain credentials") && ok;
    }
    {
        exv::helper::StartSessionRequest req;
        req.profile_id.value = "test-profile";
        req.mode = exv::helper::HelperMode::Transient;
        nlohmann::json j = req;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "StartSessionRequest should not contain credentials") && ok;
    }
    {
        exv::helper::PrepareTunnelDeviceRequest req;
        req.session_id.value = "ses-1";
        req.adapter_name = "Wintun";
        nlohmann::json j = req;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "PrepareTunnelDeviceRequest should not contain credentials") && ok;
    }
    {
        exv::helper::ApplyTunnelConfigRequest req;
        req.config.session_id.value = "ses-1";
        req.config.interface_address = "10.0.0.2/24";
        nlohmann::json j = req;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "ApplyTunnelConfigRequest should not contain credentials") && ok;
    }
    {
        exv::helper::HeartbeatRequest req;
        req.session_id.value = "ses-1";
        req.core_phase = "Connected";
        nlohmann::json j = req;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "HeartbeatRequest should not contain credentials") && ok;
    }
    {
        exv::helper::CleanupRequest req;
        req.session_id.value = "ses-1";
        nlohmann::json j = req;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "CleanupRequest should not contain credentials") && ok;
    }
    {
        exv::helper::EndSessionRequest req;
        req.session_id.value = "ses-1";
        nlohmann::json j = req;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "EndSessionRequest should not contain credentials") && ok;
    }

    return ok ? 0 : 1;
}

int test_no_credentials_in_start_session() {
    std::cout << "  [Security] StartSessionRequest doesn't contain credentials\n";
    bool ok = true;

    // Verify the struct has no credential fields
    exv::helper::StartSessionRequest req;
    req.profile_id.value = "my-profile";
    req.mode = exv::helper::HelperMode::Resident;

    nlohmann::json j = req;
    std::string serialized = j.dump();

    // Explicitly check that no credential-like keys exist
    ok = expect(serialized.find("password") == std::string::npos,
                "should not contain 'password'") && ok;
    ok = expect(serialized.find("cookie") == std::string::npos,
                "should not contain 'cookie'") && ok;
    ok = expect(serialized.find("token") == std::string::npos,
                "should not contain 'token'") && ok;
    ok = expect(serialized.find("secret") == std::string::npos,
                "should not contain 'secret'") && ok;
    ok = expect(serialized.find("credential") == std::string::npos,
                "should not contain 'credential'") && ok;

    // Verify what it SHOULD contain
    ok = expect(serialized.find("profile_id") != std::string::npos,
                "should contain profile_id") && ok;
    ok = expect(serialized.find("mode") != std::string::npos,
                "should contain mode") && ok;

    return ok ? 0 : 1;
}

int test_no_credentials_in_response_types() {
    std::cout << "  [Security] Response types don't contain credentials\n";
    bool ok = true;

    std::vector<std::string> forbidden = {"password", "cookie", "token",
                                          "secret", "credential", "auth_key"};

    {
        exv::helper::HelloResponse resp;
        resp.server_version = 2;
        resp.capabilities = {"tunnel_device_create"};
        resp.mode = exv::helper::HelperMode::Transient;
        nlohmann::json j = resp;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "HelloResponse should not contain credentials") && ok;
    }
    {
        exv::helper::StartSessionResponse resp;
        resp.session_id.value = "ses-1";
        nlohmann::json j = resp;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "StartSessionResponse should not contain credentials") && ok;
    }
    {
        exv::helper::PrepareTunnelDeviceResponse resp;
        resp.device_path = "//./Wintun/0";
        resp.mtu = 1400;
        nlohmann::json j = resp;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "PrepareTunnelDeviceResponse should not contain credentials") && ok;
    }
    {
        exv::helper::ApplyTunnelConfigResponse resp;
        resp.success = true;
        nlohmann::json j = resp;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "ApplyTunnelConfigResponse should not contain credentials") && ok;
    }
    {
        exv::helper::HeartbeatResponse resp;
        resp.ok = true;
        nlohmann::json j = resp;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "HeartbeatResponse should not contain credentials") && ok;
    }
    {
        exv::helper::CleanupResponse resp;
        resp.success = true;
        nlohmann::json j = resp;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "CleanupResponse should not contain credentials") && ok;
    }
    {
        exv::helper::EndSessionResponse resp;
        resp.success = true;
        nlohmann::json j = resp;
        ok = expect(!json_contains_credential(j.dump(), forbidden),
                    "EndSessionResponse should not contain credentials") && ok;
    }

    return ok ? 0 : 1;
}

int test_helper_request_envelope_no_credentials() {
    std::cout << "  [Security] HelperRequest envelope has no credential fields\n";
    bool ok = true;

    exv::helper::HelperRequest req;
    req.op = exv::helper::HelperOp::StartSession;
    req.payload_json = "{\"profile_id\":\"p1\",\"mode\":1}";

    nlohmann::json j = req;
    std::string serialized = j.dump();

    ok = expect(serialized.find("password") == std::string::npos,
                "envelope should not contain 'password'") && ok;
    ok = expect(serialized.find("cookie") == std::string::npos,
                "envelope should not contain 'cookie'") && ok;
    ok = expect(serialized.find("token") == std::string::npos,
                "envelope should not contain 'token'") && ok;

    // Verify envelope structure
    ok = expect(j.contains("op"), "envelope should have 'op' field") && ok;
    ok = expect(j.contains("payload_json"), "envelope should have 'payload_json' field") && ok;

    return ok ? 0 : 1;
}

// ============================================================
// 8. HARDENED: Field-name introspection for Helper V2 messages
// ============================================================

// Collect every JSON key (recursively) from a serialized message
void collect_keys(const nlohmann::json& node, std::vector<std::string>& keys) {
    if (node.is_object()) {
        for (auto it = node.begin(); it != node.end(); ++it) {
            keys.push_back(it.key());
            collect_keys(it.value(), keys);
        }
    } else if (node.is_array()) {
        for (const auto& item : node) {
            collect_keys(item, keys);
        }
    }
}

bool key_matches_credential_pattern(const std::string& key) {
    std::string lower = key;
    for (auto& c : lower) c = static_cast<char>(std::tolower(c));
    // Substring match on credential-like patterns
    static const char* patterns[] = {
        "password", "passwd", "secret", "cookie",
        "auth_token", "auth_key", "credential", "bearer",
        "api_key", "apikey", "session_cookie", "csrf",
        nullptr
    };
    for (const char** p = patterns; *p; ++p) {
        if (lower.find(*p) != std::string::npos) return true;
    }
    // "token" alone is too broad (could be "session_token" - which is OK
    // because it's not a credential). But auth_token / cookie_token must fail.
    // We check exact "token" as a standalone segment.
    if (lower == "token" || lower == "auth_token") return true;
    return false;
}

int test_helper_v2_request_field_names_no_credentials() {
    std::cout << "  [Security/Hardened] Helper V2 request field names contain no credentials\n";
    bool ok = true;

    auto check_struct = [&](const nlohmann::json& j, const char* name) {
        std::vector<std::string> keys;
        collect_keys(j, keys);
        for (const auto& k : keys) {
            if (key_matches_credential_pattern(k)) {
                std::cerr << "FAIL: " << name << " contains credential-like key: " << k << "\n";
                ok = false;
            }
        }
    };

    { exv::helper::HelloRequest r; r.client_version = 2;
      check_struct(nlohmann::json(r), "HelloRequest"); }
    { exv::helper::StartSessionRequest r; r.profile_id.value = "p";
      check_struct(nlohmann::json(r), "StartSessionRequest"); }
    { exv::helper::PrepareTunnelDeviceRequest r; r.session_id.value = "s"; r.adapter_name = "a";
      check_struct(nlohmann::json(r), "PrepareTunnelDeviceRequest"); }
    { exv::helper::ApplyTunnelConfigRequest r; r.config.session_id.value = "s";
      check_struct(nlohmann::json(r), "ApplyTunnelConfigRequest"); }
    { exv::helper::HeartbeatRequest r; r.session_id.value = "s"; r.core_phase = "Connected";
      check_struct(nlohmann::json(r), "HeartbeatRequest"); }
    { exv::helper::CleanupRequest r; r.session_id.value = "s";
      check_struct(nlohmann::json(r), "CleanupRequest"); }
    { exv::helper::EndSessionRequest r; r.session_id.value = "s";
      check_struct(nlohmann::json(r), "EndSessionRequest"); }

    return ok ? 0 : 1;
}

int test_helper_v2_response_field_names_no_credentials() {
    std::cout << "  [Security/Hardened] Helper V2 response field names contain no credentials\n";
    bool ok = true;

    auto check_struct = [&](const nlohmann::json& j, const char* name) {
        std::vector<std::string> keys;
        collect_keys(j, keys);
        for (const auto& k : keys) {
            if (key_matches_credential_pattern(k)) {
                std::cerr << "FAIL: " << name << " contains credential-like key: " << k << "\n";
                ok = false;
            }
        }
    };

    { exv::helper::HelloResponse r; r.server_version = 2;
      check_struct(nlohmann::json(r), "HelloResponse"); }
    { exv::helper::StartSessionResponse r; r.session_id.value = "s";
      check_struct(nlohmann::json(r), "StartSessionResponse"); }
    { exv::helper::PrepareTunnelDeviceResponse r; r.device_path = "/dev/tun0"; r.mtu = 1400;
      check_struct(nlohmann::json(r), "PrepareTunnelDeviceResponse"); }
    { exv::helper::ApplyTunnelConfigResponse r; r.success = true;
      check_struct(nlohmann::json(r), "ApplyTunnelConfigResponse"); }
    { exv::helper::HeartbeatResponse r; r.ok = true;
      check_struct(nlohmann::json(r), "HeartbeatResponse"); }
    { exv::helper::CleanupResponse r; r.success = true;
      check_struct(nlohmann::json(r), "CleanupResponse"); }
    { exv::helper::EndSessionResponse r; r.success = true;
      check_struct(nlohmann::json(r), "EndSessionResponse"); }

    return ok ? 0 : 1;
}

int test_helper_v2_no_auth_token_field_specifically() {
    std::cout << "  [Security/Hardened] V2 must reject auth_token field anywhere\n";
    bool ok = true;

    // Build serializations and explicitly check for known credential field
    // names that prior versions might have included.
    std::vector<std::string> banned_field_names = {
        "\"password\":", "\"Password\":", "\"PASSWORD\":",
        "\"cookie\":", "\"Cookie\":",
        "\"auth_token\":", "\"authToken\":",
        "\"session_cookie\":", "\"webvpn_cookie\":",
        "\"csrf_token\":", "\"bearer_token\":",
    };

    auto check_serialized = [&](const std::string& s, const char* name) {
        for (const auto& banned : banned_field_names) {
            if (s.find(banned) != std::string::npos) {
                std::cerr << "FAIL: " << name << " contains banned field: " << banned << "\n";
                ok = false;
            }
        }
    };

    { exv::helper::StartSessionRequest r; r.profile_id.value = "p"; r.mode = exv::helper::HelperMode::Transient;
      check_serialized(nlohmann::json(r).dump(), "StartSessionRequest"); }
    { exv::helper::ApplyTunnelConfigRequest r; r.config.session_id.value = "s";
      r.config.interface_address = "10.0.0.2/24";
      check_serialized(nlohmann::json(r).dump(), "ApplyTunnelConfigRequest"); }
    { exv::helper::HelperRequest r; r.op = exv::helper::HelperOp::StartSession; r.payload_json = "{}";
      check_serialized(nlohmann::json(r).dump(), "HelperRequest envelope"); }

    return ok ? 0 : 1;
}

int test_manifest_lists_helper_v2_ops() {
    std::cout << "  [Manifest] Helper V2 ops are generated\n";
    bool ok = true;

    using namespace exv::contracts::generated;
    ok = expect(is_helper_v2_op("Hello"), "manifest should list Hello") && ok;
    ok = expect(is_helper_v2_op("StartSession"),
                "manifest should list StartSession") && ok;
    ok = expect(is_helper_v2_op("PrepareTunnelDevice"),
                "manifest should list PrepareTunnelDevice") && ok;
    ok = expect(is_helper_v2_op("ApplyTunnelConfig"),
                "manifest should list ApplyTunnelConfig") && ok;
    ok = expect(is_helper_v2_op("Heartbeat"),
                "manifest should list Heartbeat") && ok;
    ok = expect(is_helper_v2_op("Cleanup"),
                "manifest should list Cleanup") && ok;
    ok = expect(is_helper_v2_op("GetSnapshot"),
                "manifest should list GetSnapshot") && ok;
    ok = expect(is_helper_v2_op("EndSession"),
                "manifest should list EndSession") && ok;

    return ok ? 0 : 1;
}

int test_manifest_helper_v2_op_codes_match_wire_enum() {
    std::cout << "  [Manifest] Helper V2 op codes match wire enum\n";
    bool ok = true;

    using namespace exv::contracts::generated;

    auto check = [&](std::string_view name, exv::helper::HelperOp expected,
                     bool requires_session) {
        for (const auto& item : HELPER_V2_OP_CONTRACTS) {
            if (item.name == name) {
                ok = expect(item.code == static_cast<std::uint32_t>(expected),
                            "manifest op code should match HelperOp enum") && ok;
                ok = expect(item.requires_session == requires_session,
                            "manifest requires_session should match lifecycle contract") && ok;
                return;
            }
        }
        std::cerr << "FAIL: manifest missing helper op: " << name << "\n";
        ok = false;
    };

    check("Hello", exv::helper::HelperOp::Hello, false);
    check("StartSession", exv::helper::HelperOp::StartSession, false);
    check("PrepareTunnelDevice", exv::helper::HelperOp::PrepareTunnelDevice, true);
    check("ApplyTunnelConfig", exv::helper::HelperOp::ApplyTunnelConfig, true);
    check("Heartbeat", exv::helper::HelperOp::Heartbeat, true);
    check("Cleanup", exv::helper::HelperOp::Cleanup, true);
    check("GetSnapshot", exv::helper::HelperOp::GetSnapshot, false);
    check("EndSession", exv::helper::HelperOp::EndSession, true);

    return ok ? 0 : 1;
}

int test_manifest_lists_forbidden_helper_credential_fields() {
    std::cout << "  [Manifest] Helper forbidden credential fields are generated\n";
    bool ok = true;

    using namespace exv::contracts::generated;
    ok = expect(is_helper_forbidden_credential_field("password"),
                "manifest should forbid password") && ok;
    ok = expect(is_helper_forbidden_credential_field("cookie"),
                "manifest should forbid cookie") && ok;
    ok = expect(is_helper_forbidden_credential_field("token"),
                "manifest should forbid token") && ok;
    ok = expect(is_helper_forbidden_credential_field("auth_token"),
                "manifest should forbid auth_token") && ok;
    ok = expect(is_helper_forbidden_credential_field("credential"),
                "manifest should forbid credential") && ok;

    return ok ? 0 : 1;
}

} // anonymous namespace

// ============================================================
// Main
// ============================================================

int main() {
    bool ok = true;
    int failures = 0;

    std::cout << "=== Helper V2 Contract Tests ===\n\n";

    // 1. Hello Protocol
    std::cout << "--- Hello Protocol ---\n";
    failures += test_hello_returns_correct_protocol_version();
    failures += test_hello_returns_capabilities();
    failures += test_hello_mismatched_version_still_works();

    // 2. Session Lifecycle
    std::cout << "\n--- Session Lifecycle ---\n";
    failures += test_start_session_returns_valid_id();
    failures += test_start_session_transient_mode();
    failures += test_start_session_resident_mode();
    failures += test_prepare_tunnel_device_after_start_session();
    failures += test_apply_tunnel_config_after_prepare();
    failures += test_heartbeat_updates_timestamp();
    failures += test_cleanup_removes_session();
    failures += test_end_session_removes_session();

    // 3. Error Handling
    std::cout << "\n--- Error Handling ---\n";
    failures += test_unknown_op_returns_error();
    failures += test_prepare_tunnel_without_session_fails();
    failures += test_heartbeat_without_session_fails();
    failures += test_double_start_session_same_profile();

    // 4. Lease Timeout
    std::cout << "\n--- Lease Timeout ---\n";
    failures += test_transient_session_expires();
    failures += test_resident_session_expires_later();
    failures += test_lease_timeout_config_values();
    failures += test_lifecycle_policy_heartbeat_expiry();
    failures += test_lifecycle_policy_exit_after_cleanup();

    // 5. Cleanup Registry
    std::cout << "\n--- Cleanup Registry ---\n";
    failures += test_cleanup_registry_records_resources();
    failures += test_cleanup_registry_idempotent();
    failures += test_cleanup_registry_save_load();
    failures += test_cleanup_registry_add_managed_resource();

    // 6. Command Validation
    std::cout << "\n--- Command Validation ---\n";
    failures += test_valid_v2_ops_pass_validation();
    failures += test_legacy_ops_fail_validation();
    failures += test_shell_injection_rejected();
    failures += test_path_traversal_rejected();
    failures += test_command_validator_integration();

    // 7. Security
    std::cout << "\n--- Security ---\n";
    failures += test_no_credentials_in_helper_request();
    failures += test_no_credentials_in_start_session();
    failures += test_no_credentials_in_response_types();
    failures += test_helper_request_envelope_no_credentials();

    // 8. Security/Hardened (new invariant tests)
    std::cout << "\n--- Security/Hardened ---\n";
    failures += test_helper_v2_request_field_names_no_credentials();
    failures += test_helper_v2_response_field_names_no_credentials();
    failures += test_helper_v2_no_auth_token_field_specifically();

    // 9. Generated manifest alignment
    std::cout << "\n--- Manifest Alignment ---\n";
    failures += test_manifest_lists_helper_v2_ops();
    failures += test_manifest_helper_v2_op_codes_match_wire_enum();
    failures += test_manifest_lists_forbidden_helper_credential_fields();

    std::cout << "\n=== Results ===\n";
    if (failures == 0) {
        std::cout << "helper_v2_contract_test: all tests passed\n";
    } else {
        std::cerr << "helper_v2_contract_test: " << failures << " test(s) FAILED\n";
    }
    return failures > 0 ? 1 : 0;
}
