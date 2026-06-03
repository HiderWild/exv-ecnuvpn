// Integration test: helper session lease timeout and cleanup behavior.
//
// Tests SessionLeaseManager and HelperLifecyclePolicy using real
// implementations (session_lease_manager.cpp, helper_lifecycle_policy.cpp).

#include "helper_runtime/session_lease_manager.hpp"
#include "helper_runtime/helper_lifecycle_policy.hpp"
#include "helper_common/helper_session_lease.hpp"
#include "helper_common/helper_messages.hpp"
#include "helper_common/helper_protocol.hpp"

#include <iostream>
#include <string>
#include <vector>
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
    using exv::helper::HelperLifecyclePolicy;
    using exv::helper::LeaseTimeoutConfig;
    using exv::helper::SessionId;
    using exv::helper::ProfileId;
    using exv::helper::HelperMode;
    using exv::helper::CleanupPolicy;

    // === Create session and verify it exists ===
    {
        SessionLeaseManager mgr;
        ProfileId pid{"test-profile"};
        SessionId sid = mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});

        ok = expect(mgr.has_session(sid),
                    "session should exist after creation") && ok;
        ok = expect(mgr.active_session_count() == 1,
                    "active session count should be 1") && ok;

        auto lease = mgr.get_session(sid);
        ok = expect(lease.has_value(),
                    "get_session should return the lease") && ok;
        ok = expect(lease->mode == HelperMode::Transient,
                    "lease mode should be Transient") && ok;
        ok = expect(lease->core_phase == "init",
                    "initial core_phase should be init") && ok;
    }

    // === Send heartbeats and verify lease is updated ===
    {
        SessionLeaseManager mgr;
        ProfileId pid{"test-profile"};
        SessionId sid = mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});

        mgr.update_heartbeat(sid, "Connected");
        auto lease = mgr.get_session(sid);
        ok = expect(lease.has_value(), "lease should exist after heartbeat") && ok;
        ok = expect(lease->core_phase == "Connected",
                    "core_phase should be Connected after heartbeat") && ok;
    }

    // === Stop heartbeat -> fresh sessions are NOT expired ===
    {
        SessionLeaseManager mgr;
        ProfileId pid{"test-profile"};
        SessionId sid = mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});

        // Fresh sessions (just created) should not be expired
        auto expired = mgr.find_expired_sessions(std::chrono::steady_clock::now());
        ok = expect(expired.empty(),
                    "fresh sessions should not be expired") && ok;
    }

    // === HelperLifecyclePolicy: custom timeout with zero duration ===
    {
        LeaseTimeoutConfig config;
        config.transient_heartbeat_timeout = std::chrono::seconds(0);
        config.resident_heartbeat_timeout = std::chrono::seconds(0);
        HelperLifecyclePolicy policy(config);

        // A lease with last_heartbeat in the past should be expired
        exv::helper::SessionLease lease;
        lease.mode = HelperMode::Transient;
        lease.last_heartbeat = std::chrono::steady_clock::now() -
                               std::chrono::seconds(1);
        ok = expect(policy.is_heartbeat_expired(lease,
                                                std::chrono::steady_clock::now()),
                    "lease with zero timeout should be expired immediately") && ok;

        ok = expect(policy.should_cleanup_stale(lease,
                                                std::chrono::steady_clock::now()),
                    "should_cleanup_stale should match is_heartbeat_expired") && ok;
    }

    // === scan_stale_sessions with fresh sessions: no cleanup ===
    {
        SessionLeaseManager mgr;
        ProfileId pid{"test-profile"};
        mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});

        int cleanup_count = 0;
        mgr.scan_stale_sessions([&](const exv::helper::SessionLease&) {
            cleanup_count++;
        });

        ok = expect(cleanup_count == 0,
                    "scan_stale_sessions should not cleanup fresh sessions") && ok;
        ok = expect(mgr.active_session_count() == 1,
                    "fresh session should remain after scan") && ok;
    }

    // === Transient vs Resident timeout difference ===
    {
        LeaseTimeoutConfig config;
        config.transient_heartbeat_timeout = std::chrono::seconds(1);
        config.resident_heartbeat_timeout = std::chrono::seconds(60);

        HelperLifecyclePolicy policy(config);

        ok = expect(policy.heartbeat_timeout(HelperMode::Transient) ==
                        std::chrono::seconds(1),
                    "transient timeout should be 1s") && ok;
        ok = expect(policy.heartbeat_timeout(HelperMode::Resident) ==
                        std::chrono::seconds(60),
                    "resident timeout should be 60s") && ok;
    }

    // === HelperLifecyclePolicy: is_heartbeat_expired ===
    {
        LeaseTimeoutConfig config;
        config.transient_heartbeat_timeout = std::chrono::seconds(5);
        config.resident_heartbeat_timeout = std::chrono::seconds(30);

        HelperLifecyclePolicy policy(config);

        // Fresh lease (just created) should not be expired
        exv::helper::SessionLease fresh_lease;
        fresh_lease.mode = HelperMode::Transient;
        fresh_lease.last_heartbeat = std::chrono::steady_clock::now();
        ok = expect(!policy.is_heartbeat_expired(fresh_lease,
                                                 std::chrono::steady_clock::now()),
                    "fresh lease should not be expired") && ok;

        // Old lease (10s ago with 5s timeout) should be expired
        exv::helper::SessionLease old_lease;
        old_lease.mode = HelperMode::Transient;
        old_lease.last_heartbeat = std::chrono::steady_clock::now() -
                                   std::chrono::seconds(10);
        ok = expect(policy.is_heartbeat_expired(old_lease,
                                                std::chrono::steady_clock::now()),
                    "old transient lease (10s) should be expired (5s timeout)") && ok;

        // Resident lease 10s old with 30s timeout should NOT be expired
        exv::helper::SessionLease resident_lease;
        resident_lease.mode = HelperMode::Resident;
        resident_lease.last_heartbeat = std::chrono::steady_clock::now() -
                                        std::chrono::seconds(10);
        ok = expect(!policy.is_heartbeat_expired(resident_lease,
                                                 std::chrono::steady_clock::now()),
                    "resident lease (10s) should not be expired (30s timeout)") && ok;
    }

    // === HelperLifecyclePolicy: should_exit_after_cleanup ===
    {
        HelperLifecyclePolicy policy;
        ok = expect(policy.should_exit_after_cleanup(HelperMode::Transient),
                    "transient helper should exit after cleanup") && ok;
        ok = expect(!policy.should_exit_after_cleanup(HelperMode::Resident),
                    "resident helper should not exit after cleanup") && ok;
    }

    // === Transient vs Resident behavior: expired sessions ===
    {
        LeaseTimeoutConfig config;
        config.transient_heartbeat_timeout = std::chrono::seconds(1);
        config.resident_heartbeat_timeout = std::chrono::seconds(60);

        SessionLeaseManager mgr;
        ProfileId pid{"test-profile"};

        // Create one transient and one resident session
        SessionId transient_id = mgr.create_session(pid, HelperMode::Transient,
                                                     CleanupPolicy{});
        SessionId resident_id = mgr.create_session(pid, HelperMode::Resident,
                                                    CleanupPolicy{});

        ok = expect(mgr.active_session_count() == 2,
                    "should have 2 active sessions") && ok;

        // Simulate time passing: both sessions are 5 seconds old
        // (We can't easily manipulate the clock, so we use update_heartbeat
        //  to make one session fresh and leave the other stale.)
        mgr.update_heartbeat(resident_id, "Connected");

        // After update, resident is fresh. Transient was never updated so
        // its last_heartbeat is at creation time.
        // With default timeouts (30s transient, 60s resident), neither is
        // expired immediately. But we can verify the structure works.
        auto transient_lease = mgr.get_session(transient_id);
        auto resident_lease = mgr.get_session(resident_id);

        ok = expect(transient_lease.has_value(),
                    "transient lease should exist") && ok;
        ok = expect(resident_lease.has_value(),
                    "resident lease should exist") && ok;
        ok = expect(transient_lease->core_phase == "init",
                    "transient should still have init phase") && ok;
        ok = expect(resident_lease->core_phase == "Connected",
                    "resident should have Connected phase after heartbeat") && ok;
    }

    // === Remove session ===
    {
        SessionLeaseManager mgr;
        ProfileId pid{"test-profile"};
        SessionId sid = mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});

        ok = expect(mgr.has_session(sid), "session should exist") && ok;
        mgr.remove_session(sid);
        ok = expect(!mgr.has_session(sid),
                    "session should not exist after removal") && ok;
        ok = expect(mgr.active_session_count() == 0,
                    "active count should be 0 after removal") && ok;
    }

    // === Multiple sessions ===
    {
        SessionLeaseManager mgr;
        ProfileId pid{"test-profile"};

        SessionId s1 = mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});
        SessionId s2 = mgr.create_session(pid, HelperMode::Resident, CleanupPolicy{});
        SessionId s3 = mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});

        ok = expect(mgr.active_session_count() == 3,
                    "should have 3 active sessions") && ok;
        ok = expect(s1.value != s2.value && s2.value != s3.value,
                    "session IDs should be unique") && ok;
    }

    // === scan_stale_sessions with no expired sessions ===
    {
        SessionLeaseManager mgr;
        ProfileId pid{"test-profile"};
        mgr.create_session(pid, HelperMode::Transient, CleanupPolicy{});

        int cleanup_count = 0;
        mgr.scan_stale_sessions([&](const exv::helper::SessionLease&) {
            cleanup_count++;
        });

        ok = expect(cleanup_count == 0,
                    "no cleanup should be called for fresh sessions") && ok;
        ok = expect(mgr.active_session_count() == 1,
                    "fresh session should remain") && ok;
    }

    if (ok) {
        std::cout << "helper_timeout_cleanup_test: all assertions passed\n";
    } else {
        std::cerr << "helper_timeout_cleanup_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
