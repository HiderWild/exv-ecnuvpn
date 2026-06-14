#pragma once
#include "helper/common/helper_messages.hpp"
#include "helper/helper_network_ops.hpp"
#include "helper/runtime/helper_request_dispatcher.hpp"
#include "helper/runtime/session_lease_manager.hpp"
#include "helper/runtime/cleanup_registry.hpp"
#include "helper/runtime/helper_lifecycle_policy.hpp"
#include "helper/runtime/command_validator.hpp"
#include <memory>

namespace exv::helper {

class HelperHandler {
public:
    explicit HelperHandler(HelperLifecyclePolicy policy = HelperLifecyclePolicy());
    HelperHandler(HelperLifecyclePolicy policy,
                  std::shared_ptr<HelperNetworkOps> network_ops);

    HelperResponse handle(const HelperRequest& request);

    // Periodic maintenance (check timeouts, cleanup stale sessions)
    void tick();

    // Access internals for testing
    SessionLeaseManager& lease_manager();
    CleanupRegistry& cleanup_registry();
    bool should_stop() const;
    void set_startup_context(HelperStartupContext context);
    CleanupResponse cleanup_all_sessions(const CleanupPolicy& policy);

private:
    void register_handlers();
    CleanupResponse cleanup_session(const SessionId& session_id,
                                    const CleanupPolicy& policy);

    HelperResponse handle_hello(const HelperRequest& req);
    HelperResponse handle_start_session(const HelperRequest& req);
    HelperResponse handle_prepare_tunnel_device(const HelperRequest& req);
    HelperResponse handle_apply_tunnel_config(const HelperRequest& req);
    HelperResponse handle_heartbeat(const HelperRequest& req);
    HelperResponse handle_cleanup(const HelperRequest& req);
    HelperResponse handle_get_snapshot(const HelperRequest& req);
    HelperResponse handle_shutdown(const HelperRequest& req);

    HelperRequestDispatcher dispatcher_;
    SessionLeaseManager leases_;
    CleanupRegistry cleanup_;
    HelperLifecyclePolicy policy_;
    CommandValidator validator_;
    HelperStartupContext startup_context_;
    std::shared_ptr<HelperNetworkOps> network_ops_;
    bool shutdown_requested_ = false;
};

} // namespace exv::helper
