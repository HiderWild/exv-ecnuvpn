#pragma once
#include <chrono>
#include <string>
#include "helper_protocol.hpp"
#include "helper_messages.hpp"

namespace exv::helper {

struct SessionLease {
    SessionId session_id;
    ProfileId profile_id;
    HelperMode mode = HelperMode::Transient;
    std::chrono::steady_clock::time_point last_heartbeat;
    std::string core_phase;  // Current TunnelPhase from core
    CleanupPolicy cleanup_policy;
};

// Lease timeout configuration
struct LeaseTimeoutConfig {
    // Transient: how long to wait for heartbeat before cleanup + exit
    std::chrono::seconds transient_heartbeat_timeout{30};
    // Resident: how long to wait for heartbeat before cleanup (no exit)
    std::chrono::seconds resident_heartbeat_timeout{60};
    // How long after cleanup before transient helper exits
    std::chrono::seconds transient_idle_timeout{5};
    // Max reconnect window - core must reconnect within this time
    std::chrono::seconds max_reconnect_window{300};
};

} // namespace exv::helper
