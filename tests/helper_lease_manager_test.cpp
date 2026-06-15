// Tests for SessionLeaseManager: session lifecycle, heartbeat, expiry.

#include "helper/runtime/session_lease_manager.hpp"
#include "helper/common/helper_session_lease.hpp"
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_protocol.hpp"

#include <iostream>
#include <string>
#include <thread>
#include <chrono>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

} // namespace

int main() {
    bool ok = true;

    using exv::helper::SessionLeaseManager;
    using exv::helper::SessionId;
    using exv::helper::ProfileId;
    using exv::helper::HelperMode;
    using exv::helper::CleanupPolicy;

    // --- create_session returns a valid session ---
    {
        SessionLeaseManager mgr;
        ProfileId pid{"profile-1"};
        SessionId sid = mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});

        ok = expect(!sid.value.empty(),
                    "created session ID should not be empty") && ok;
        ok = expect(mgr.has_session(sid),
                    "session should exist after creation") && ok;
        ok = expect(mgr.active_session_count() == 1,
                    "active count should be 1") && ok;
    }

    // --- get_session returns correct lease ---
    {
        SessionLeaseManager mgr;
        ProfileId pid{"profile-1"};
        SessionId sid = mgr.create_session(pid, HelperMode::Resident, CleanupPolicy{});

        auto lease = mgr.get_session(sid);
        ok = expect(lease.has_value(), "get_session should return lease") && ok;
        ok = expect(lease->session_id == sid, "lease session_id should match") && ok;
        ok = expect(lease->profile_id.value == "profile-1",
                    "lease profile_id should match") && ok;
        ok = expect(lease->mode == HelperMode::Resident,
                    "lease mode should be Resident") && ok;
        ok = expect(lease->core_phase == "init",
                    "initial core_phase should be init") && ok;
    }

    // --- get_session returns nullopt for unknown ID ---
    {
        SessionLeaseManager mgr;
        SessionId unknown;
        unknown.value = "nonexistent";
        auto lease = mgr.get_session(unknown);
        ok = expect(!lease.has_value(),
                    "get_session for unknown ID should return nullopt") && ok;
    }

    // --- update_heartbeat updates phase and timestamp ---
    {
        SessionLeaseManager mgr;
        SessionId sid = mgr.create_session({"p"}, HelperMode::Transient, CleanupPolicy{});

        auto before = mgr.get_session(sid)->last_heartbeat;
        mgr.update_heartbeat(sid, "Connected");
        auto after = mgr.get_session(sid);

        ok = expect(after->core_phase == "Connected",
                    "core_phase should be Connected after update") && ok;
        ok = expect(after->last_heartbeat >= before,
                    "last_heartbeat should be updated") && ok;
    }

    // --- update_heartbeat on unknown session is a no-op ---
    {
        SessionLeaseManager mgr;
        SessionId unknown;
        unknown.value = "nonexistent";
        mgr.update_heartbeat(unknown, "Connected");  // should not crash
        ok = expect(mgr.active_session_count() == 0,
                    "no sessions should be created by update_heartbeat") && ok;
    }

    // --- remove_session removes the session ---
    {
        SessionLeaseManager mgr;
        SessionId sid = mgr.create_session({"p"}, HelperMode::Transient, CleanupPolicy{});
        ok = expect(mgr.has_session(sid), "session should exist") && ok;

        mgr.remove_session(sid);
        ok = expect(!mgr.has_session(sid),
                    "session should not exist after removal") && ok;
        ok = expect(mgr.active_session_count() == 0,
                    "active count should be 0 after removal") && ok;
    }

    // --- find_expired_sessions with fresh sessions returns empty ---
    {
        SessionLeaseManager mgr;
        mgr.create_session({"p"}, HelperMode::Transient, CleanupPolicy{});
        mgr.create_session({"p"}, HelperMode::Resident, CleanupPolicy{});

        auto expired = mgr.find_expired_sessions(std::chrono::steady_clock::now());
        ok = expect(expired.empty(),
                    "fresh sessions should not be expired") && ok;
    }

    // --- scan_stale_sessions with fresh sessions: no cleanup ---
    {
        SessionLeaseManager mgr;
        mgr.create_session({"p"}, HelperMode::Transient, CleanupPolicy{});

        int cleanup_count = 0;
        mgr.scan_stale_sessions([&](const exv::helper::SessionLease&) {
            cleanup_count++;
        });
        ok = expect(cleanup_count == 0,
                    "fresh sessions should not trigger cleanup") && ok;
        ok = expect(mgr.active_session_count() == 1,
                    "fresh session should remain") && ok;
    }

    // --- Multiple sessions: unique IDs ---
    {
        SessionLeaseManager mgr;
        SessionId s1 = mgr.create_session({"p1"}, HelperMode::Transient, CleanupPolicy{});
        SessionId s2 = mgr.create_session({"p2"}, HelperMode::Transient, CleanupPolicy{});
        SessionId s3 = mgr.create_session({"p3"}, HelperMode::Resident, CleanupPolicy{});

        ok = expect(s1.value != s2.value, "session IDs should be unique (1 vs 2)") && ok;
        ok = expect(s2.value != s3.value, "session IDs should be unique (2 vs 3)") && ok;
        ok = expect(mgr.active_session_count() == 3,
                    "should have 3 active sessions") && ok;
    }

    // --- CleanupPolicy is stored correctly ---
    {
        SessionLeaseManager mgr;
        CleanupPolicy policy;
        policy.remove_routes = true;
        policy.remove_dns = false;
        policy.remove_adapter = true;
        policy.remove_firewall_rules = false;

        SessionId sid = mgr.create_session({"p"}, HelperMode::Transient, policy);
        auto lease = mgr.get_session(sid);

        ok = expect(lease->cleanup_policy.remove_routes,
                    "remove_routes should be true") && ok;
        ok = expect(!lease->cleanup_policy.remove_dns,
                    "remove_dns should be false") && ok;
        ok = expect(lease->cleanup_policy.remove_adapter,
                    "remove_adapter should be true") && ok;
        ok = expect(!lease->cleanup_policy.remove_firewall_rules,
                    "remove_firewall_rules should be false") && ok;
    }

    if (ok) {
        std::cout << "helper_lease_manager_test: all assertions passed\n";
    } else {
        std::cerr << "helper_lease_manager_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
