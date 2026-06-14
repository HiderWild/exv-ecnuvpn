#pragma once
#include "helper/common/helper_session_lease.hpp"
#include <map>
#include <optional>
#include <functional>
#include <vector>

namespace exv::helper {

class SessionLeaseManager {
public:
    // Session lifecycle
    SessionId create_session(ProfileId profile_id, HelperMode mode, CleanupPolicy policy);
    bool has_session(const SessionId& id) const;
    std::optional<SessionLease> get_session(const SessionId& id) const;
    void remove_session(const SessionId& id);

    // Heartbeat
    void update_heartbeat(const SessionId& id, const std::string& core_phase);

    // Find expired sessions
    std::vector<SessionId> find_expired_sessions(std::chrono::steady_clock::time_point now) const;

    // Scan and cleanup stale sessions on startup
    using CleanupHandler = std::function<void(const SessionLease&)>;
    void scan_stale_sessions(CleanupHandler cleanup_handler);

    size_t active_session_count() const;
    std::vector<SessionId> active_session_ids() const;

private:
    std::map<SessionId, SessionLease> leases_;
};

} // namespace exv::helper
