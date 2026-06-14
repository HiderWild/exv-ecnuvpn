#pragma once
#include "helper/common/helper_messages.hpp"
#include <string>
#include <vector>
#include <map>
#include <set>
#include <chrono>

namespace exv::helper {

struct ManagedResource {
    std::string type;    // "route", "dns", "adapter", "firewall_rule"
    std::string detail;  // e.g. "0.0.0.0/0 via 10.0.0.1", adapter GUID, etc.
};

struct CleanupRecord {
    SessionId session_id;
    std::string adapter_name;
    std::vector<RouteEntry> routes;
    DnsConfig dns;
    std::vector<std::string> firewall_rules;
    std::chrono::system_clock::time_point created_at;
};

class CleanupRegistry {
public:
    // Register resources created for a session
    void register_session(const CleanupRecord& record);

    // Add a managed resource
    void add_resource(const SessionId& id, const ManagedResource& resource);

    // Get all resources for a session
    std::vector<ManagedResource> get_resources(const SessionId& id) const;

    // Remove session record (after successful cleanup)
    void remove_session(const SessionId& id);

    // Get all sessions (for stale scan)
    std::vector<CleanupRecord> all_records() const;

    // Persist to disk (survives helper crash)
    bool save_to_disk(const std::string& path) const;
    bool load_from_disk(const std::string& path);

private:
    std::map<SessionId, CleanupRecord> records_;
};

} // namespace exv::helper
