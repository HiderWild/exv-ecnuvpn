#pragma once
#include "core/lifecycle/core_registry.hpp"
#include "helper/common/helper_messages.hpp"

#include <optional>
#include <string>
#include <vector>
#include <map>
#include <chrono>

namespace exv::helper {

struct CoreRegistryCleanupBinding {
    std::string registry_path;
    exv::core::lifecycle::CoreRegistryDeleteMatch delete_match;
};

struct CleanupRecord {
    SessionId session_id;
    std::string adapter_name;
    std::vector<RouteEntry> routes;
    DnsConfig dns;
    std::vector<std::string> firewall_rules;
    std::vector<ManagedResource> managed_resources;
    std::chrono::system_clock::time_point created_at;
    std::optional<CoreRegistryCleanupBinding> core_registry_cleanup;
};

class CleanupRegistry {
public:
    // Register resources created for a session
    void register_session(const CleanupRecord& record);

    // Add a managed resource
    void add_resource(const SessionId& id, const ManagedResource& resource);

    // Get all resources for a session
    std::vector<ManagedResource> get_resources(const SessionId& id) const;

    // Remove only the session record.
    void remove_session(const SessionId& id);

    // Complete successful cleanup for a session and then run its bound core
    // registry compare/delete action.
    void complete_session_cleanup(const SessionId& id);

    // Bind a versioned core registry compare-and-delete operation to the
    // session's success path.
    void bind_core_registry_cleanup(const SessionId& id,
                                    const CoreRegistryCleanupBinding& binding);

    // Get all sessions (for stale scan)
    std::vector<CleanupRecord> all_records() const;

    // Persist to disk (survives helper crash)
    bool save_to_disk(const std::string& path) const;
    bool load_from_disk(const std::string& path);

private:
    std::map<SessionId, CleanupRecord> records_;
};

} // namespace exv::helper
