#include "helper_lifecycle_policy.hpp"

namespace exv::helper {

HelperLifecyclePolicy::HelperLifecyclePolicy(
    LeaseTimeoutConfig config, std::chrono::seconds core_lease_timeout)
    : config_(config), core_lease_timeout_(core_lease_timeout) {}

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

std::chrono::seconds HelperLifecyclePolicy::core_lease_timeout() const {
    return core_lease_timeout_;
}

bool HelperLifecyclePolicy::is_core_lease_expired(
    std::chrono::steady_clock::time_point last_seen,
    std::chrono::steady_clock::time_point now) const {
    return (now - last_seen) >= core_lease_timeout_;
}

} // namespace exv::helper
