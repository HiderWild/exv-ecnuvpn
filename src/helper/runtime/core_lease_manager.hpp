#pragma once

#include "helper/common/helper_messages.hpp"

#include <chrono>
#include <limits>
#include <optional>
#include <string>

namespace exv::helper {

struct HelperPeerIdentity {
    bool verified = false;
    std::string owner;
    unsigned int uid = std::numeric_limits<unsigned int>::max();
    unsigned int gid = std::numeric_limits<unsigned int>::max();
    int pid = 0;
};

struct CoreLease {
    std::string lease_id;
    int core_pid = 0;
    std::string purpose;
    std::string last_seen_state;
    std::chrono::steady_clock::time_point last_seen;
    HelperPeerIdentity peer;
};

class CoreLeaseManager {
public:
    bool has_active_lease() const {
        return active_.has_value();
    }

    std::optional<CoreLease> active_lease() const {
        return active_;
    }

    CoreLeaseState state() const {
        CoreLeaseState result;
        if (!active_.has_value()) {
            return result;
        }
        result.active = true;
        result.lease_id = active_->lease_id;
        result.core_pid = active_->core_pid;
        result.purpose = active_->purpose;
        result.last_seen_state = active_->last_seen_state;
        return result;
    }

    bool acquire(int core_pid, const std::string& purpose,
                 const HelperPeerIdentity& peer) {
        if (active_.has_value() || core_pid <= 0 || purpose.empty()) {
            return false;
        }
        CoreLease lease;
        lease.lease_id = "core-lease-" + std::to_string(next_id_++);
        lease.core_pid = core_pid;
        lease.purpose = purpose;
        lease.last_seen_state = "acquired";
        lease.last_seen = std::chrono::steady_clock::now();
        lease.peer = peer;
        active_ = std::move(lease);
        return true;
    }

    bool keep_alive(const std::string& lease_id, const std::string& state) {
        if (!matches(lease_id)) {
            return false;
        }
        mark_activity(state);
        return true;
    }

    void mark_activity(const std::string& state) {
        if (!active_.has_value()) {
            return;
        }
        active_->last_seen = std::chrono::steady_clock::now();
        active_->last_seen_state = state;
    }

    bool release(const std::string& lease_id) {
        if (!matches(lease_id)) {
            return false;
        }
        active_.reset();
        return true;
    }

    void clear() {
        active_.reset();
    }

    bool matches(const std::string& lease_id) const {
        return active_.has_value() && active_->lease_id == lease_id;
    }

    bool peer_matches(const HelperPeerIdentity& peer) const {
        if (!active_.has_value()) {
            return false;
        }
        return peer_matches(active_->peer, peer);
    }

    static bool peer_matches(const HelperPeerIdentity& expected,
                             const HelperPeerIdentity& actual) {
        if (!expected.verified || !actual.verified) {
            return false;
        }
        if (!expected.owner.empty() || !actual.owner.empty()) {
            return !expected.owner.empty() && expected.owner == actual.owner;
        }
        if (expected.uid != std::numeric_limits<unsigned int>::max() &&
            actual.uid != std::numeric_limits<unsigned int>::max()) {
            return expected.uid == actual.uid;
        }
        if (expected.pid > 0 && actual.pid > 0) {
            return expected.pid == actual.pid;
        }
        return false;
    }

private:
    std::optional<CoreLease> active_;
    int next_id_ = 1;
};

} // namespace exv::helper
