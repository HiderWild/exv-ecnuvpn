#pragma once
#include "../helper_common/helper_protocol.hpp"
#include "../helper_common/helper_session_lease.hpp"
#include <chrono>

namespace exv::helper {

class HelperLifecyclePolicy {
public:
    explicit HelperLifecyclePolicy(LeaseTimeoutConfig config = {});

    // Should the helper exit after session cleanup?
    bool should_exit_after_cleanup(HelperMode mode) const;

    // Has heartbeat timed out?
    bool is_heartbeat_expired(const SessionLease& lease, std::chrono::steady_clock::time_point now) const;

    // Should we cleanup stale session?
    bool should_cleanup_stale(const SessionLease& lease, std::chrono::steady_clock::time_point now) const;

    // Get timeout for given mode
    std::chrono::seconds heartbeat_timeout(HelperMode mode) const;

private:
    LeaseTimeoutConfig config_;
};

} // namespace exv::helper
