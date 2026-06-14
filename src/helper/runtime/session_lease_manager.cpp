#include "session_lease_manager.hpp"

#include <algorithm>
#include <random>
#include <sstream>
#include <iomanip>

namespace exv::helper {

namespace {

std::string generate_session_id() {
    static std::random_device rd;
    static std::mt19937 gen(rd());
    std::uniform_int_distribution<uint64_t> dist;

    std::ostringstream oss;
    oss << "ses-" << std::hex << std::setfill('0')
        << std::setw(16) << dist(gen)
        << std::setw(16) << dist(gen);
    return oss.str();
}

} // anonymous namespace

SessionId SessionLeaseManager::create_session(
    ProfileId profile_id, HelperMode mode, CleanupPolicy policy) {
    SessionId id{generate_session_id()};
    SessionLease lease;
    lease.session_id = id;
    lease.profile_id = std::move(profile_id);
    lease.mode = mode;
    lease.last_heartbeat = std::chrono::steady_clock::now();
    lease.core_phase = "init";
    lease.cleanup_policy = policy;
    leases_.emplace(id, std::move(lease));
    return id;
}

bool SessionLeaseManager::has_session(const SessionId& id) const {
    return leases_.count(id) > 0;
}

std::optional<SessionLease> SessionLeaseManager::get_session(const SessionId& id) const {
    auto it = leases_.find(id);
    if (it == leases_.end()) return std::nullopt;
    return it->second;
}

void SessionLeaseManager::remove_session(const SessionId& id) {
    leases_.erase(id);
}

void SessionLeaseManager::update_heartbeat(const SessionId& id, const std::string& core_phase) {
    auto it = leases_.find(id);
    if (it == leases_.end()) return;
    it->second.last_heartbeat = std::chrono::steady_clock::now();
    it->second.core_phase = core_phase;
}

std::vector<SessionId> SessionLeaseManager::find_expired_sessions(
    std::chrono::steady_clock::time_point now) const {
    // Use default timeouts for the scan
    LeaseTimeoutConfig config;
    std::vector<SessionId> expired;
    for (const auto& [id, lease] : leases_) {
        auto timeout = (lease.mode == HelperMode::Transient)
            ? config.transient_heartbeat_timeout
            : config.resident_heartbeat_timeout;
        if (now - lease.last_heartbeat > timeout) {
            expired.push_back(id);
        }
    }
    return expired;
}

void SessionLeaseManager::scan_stale_sessions(CleanupHandler cleanup_handler) {
    auto now = std::chrono::steady_clock::now();
    auto expired = find_expired_sessions(now);
    for (const auto& id : expired) {
        auto it = leases_.find(id);
        if (it != leases_.end()) {
            cleanup_handler(it->second);
            leases_.erase(it);
        }
    }
}

size_t SessionLeaseManager::active_session_count() const {
    return leases_.size();
}

std::vector<SessionId> SessionLeaseManager::active_session_ids() const {
    std::vector<SessionId> ids;
    ids.reserve(leases_.size());
    for (const auto& [id, lease] : leases_) {
        (void)lease;
        ids.push_back(id);
    }
    return ids;
}

} // namespace exv::helper
