#pragma once
#include "session_lease_manager.hpp"
#include "cleanup_registry.hpp"

namespace exv::helper {

class HelperSessionStore {
public:
    SessionLeaseManager& lease_manager();
    CleanupRegistry& cleanup_registry();

    // Persistence
    bool save(const std::string& path) const;
    bool load(const std::string& path);

private:
    SessionLeaseManager leases_;
    CleanupRegistry cleanup_;
};

} // namespace exv::helper
