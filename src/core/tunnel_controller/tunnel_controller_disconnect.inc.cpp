    // ================================================================
    // Disconnect flow
    // ================================================================

    void do_disconnect(DisconnectReason reason) {
        intent_.desired_connected    = false;
        intent_.user_disconnect_reason = reason;

        stop_heartbeat();

        // Stop the native engine session if it's running.
        if (runner_.is_running()) {
            runner_.stop();
        }

        transition_to(TunnelPhase::Disconnecting);
        do_cleanup();
    }

    void shutdown_helper_session_for_cleanup() {
        try {
            exv::helper::ShutdownRequest req;
            req.session_id = session_id_;
            req.policy.remove_routes       = true;
            req.policy.remove_dns          = true;
            req.policy.remove_adapter      = true;
            req.policy.remove_firewall_rules = true;

            auto resp = helper_->shutdown(req);

            if (!resp.cleanup_success) {
                // Log per-resource errors but still finish cleanup (best effort)
            }
        } catch (const std::exception&) {
            // Cleanup threw — nothing we can do; finish best effort.
        }

        if (auto delegated_ops = as_helper_delegating_ops(net_ops_)) {
            delegated_ops->clear_session();
        }
        session_id_ = exv::helper::SessionId{};
        network_config_applied_ = false;
    }

    void cleanup_after_failed_startup() {
        stop_heartbeat();
        shutdown_helper_session_for_cleanup();
    }

    void do_cleanup() {
        stop_heartbeat();
        transition_to(TunnelPhase::CleaningUp);

        shutdown_helper_session_for_cleanup();

        transition_to(TunnelPhase::Idle);
    }
