#pragma once
#include "helper/common/helper_protocol.hpp"
#include "helper/common/helper_session_lease.hpp"
#include <chrono>

namespace exv::helper {

class HelperLifecyclePolicy {
public:
    explicit HelperLifecyclePolicy(
        LeaseTimeoutConfig config = {},
        std::chrono::seconds core_lease_timeout = std::chrono::seconds(60));

    // Should the helper exit after session cleanup?
    bool should_exit_after_cleanup(HelperMode mode) const;

    // Has heartbeat timed out?
    bool is_heartbeat_expired(const SessionLease& lease, std::chrono::steady_clock::time_point now) const;

    // Should we cleanup stale session?
    bool should_cleanup_stale(const SessionLease& lease, std::chrono::steady_clock::time_point now) const;

    // Get timeout for given mode
    std::chrono::seconds heartbeat_timeout(HelperMode mode) const;

    std::chrono::seconds core_lease_timeout() const;

    bool is_core_lease_expired(
        std::chrono::steady_clock::time_point last_seen,
        std::chrono::steady_clock::time_point now) const;

private:
    LeaseTimeoutConfig config_;
    std::chrono::seconds core_lease_timeout_;
};

} // namespace exv::helper
