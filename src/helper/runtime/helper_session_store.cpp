#include "helper_session_store.hpp"

namespace exv::helper {

SessionLeaseManager& HelperSessionStore::lease_manager() {
    return leases_;
}

CleanupRegistry& HelperSessionStore::cleanup_registry() {
    return cleanup_;
}

bool HelperSessionStore::save(const std::string& path) const {
    return cleanup_.save_to_disk(path);
}

bool HelperSessionStore::load(const std::string& path) {
    return cleanup_.load_from_disk(path);
}

} // namespace exv::helper
