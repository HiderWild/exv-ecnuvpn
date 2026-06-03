#include "helper_lifecycle_policy.hpp"

namespace exv::helper {

HelperLifecyclePolicy::HelperLifecyclePolicy(LeaseTimeoutConfig config)
    : config_(config) {}

bool HelperLifecyclePolicy::should_exit_after_cleanup(HelperMode mode) const {
    // Transient helpers exit after cleanup; Resident helpers stay alive
    return mode == HelperMode::Transient;
}

bool HelperLifecyclePolicy::is_heartbeat_expired(
    const SessionLease& lease, std::chrono::steady_clock::time_point now) const {
    auto timeout = heartbeat_timeout(lease.mode);
    return (now - lease.last_heartbeat) > timeout;
}

bool HelperLifecyclePolicy::should_cleanup_stale(
    const SessionLease& lease, std::chrono::steady_clock::time_point now) const {
    return is_heartbeat_expired(lease, now);
}

std::chrono::seconds HelperLifecyclePolicy::heartbeat_timeout(HelperMode mode) const {
    switch (mode) {
        case HelperMode::Transient:
            return config_.transient_heartbeat_timeout;
        case HelperMode::Resident:
            return config_.resident_heartbeat_timeout;
        default:
            return config_.transient_heartbeat_timeout;
    }
}

} // namespace exv::helper
