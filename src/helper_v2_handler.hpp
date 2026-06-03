#pragma once
#include "helper_common/helper_messages.hpp"
#include "helper_runtime/helper_request_dispatcher.hpp"
#include "helper_runtime/session_lease_manager.hpp"
#include "helper_runtime/cleanup_registry.hpp"
#include "helper_runtime/helper_lifecycle_policy.hpp"
#include "helper_runtime/command_validator.hpp"
#include <memory>

namespace exv::helper {

class HelperV2Handler {
public:
    HelperV2Handler();

    // Handle a V2 request
    HelperResponse handle(const HelperRequest& request);

    // Periodic maintenance (check timeouts, cleanup stale sessions)
    void tick();

    // Access internals for testing
    SessionLeaseManager& lease_manager();
    CleanupRegistry& cleanup_registry();

private:
    void register_handlers();

    HelperResponse handle_hello(const HelperRequest& req);
    HelperResponse handle_start_session(const HelperRequest& req);
    HelperResponse handle_prepare_tunnel_device(const HelperRequest& req);
    HelperResponse handle_apply_tunnel_config(const HelperRequest& req);
    HelperResponse handle_heartbeat(const HelperRequest& req);
    HelperResponse handle_cleanup(const HelperRequest& req);
    HelperResponse handle_get_snapshot(const HelperRequest& req);
    HelperResponse handle_end_session(const HelperRequest& req);

    HelperRequestDispatcher dispatcher_;
    SessionLeaseManager leases_;
    CleanupRegistry cleanup_;
    HelperLifecyclePolicy policy_;
    CommandValidator validator_;
};

} // namespace exv::helper
