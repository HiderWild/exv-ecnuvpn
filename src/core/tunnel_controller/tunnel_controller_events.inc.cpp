    // ================================================================
    // Event handlers (called from on_event)
    // ================================================================

    void on_helper_ready() {
        if (phase_ == TunnelPhase::PreparingHelper) {
            timing_.timer.start(ConnectTiming::AUTH);
            transition_to(TunnelPhase::Authenticating);
        }
    }

    void on_auth_succeeded() {
        if (phase_ == TunnelPhase::Authenticating) {
            timing_.timer.end(ConnectTiming::AUTH);
            timing_.timer.start(ConnectTiming::CSTP_CONNECT);
            transition_to(TunnelPhase::ConnectingCstp);
        }
    }

    void on_auth_failed() {
        if (phase_ == TunnelPhase::Authenticating) {
            timing_.timer.end(ConnectTiming::AUTH);
            stop_heartbeat();
            set_error(CoreErrorMapper::from_auth_error(-1, "Authentication failed"));
            transition_to(TunnelPhase::Failed);
        }
    }

    void on_cstp_connected() {
        if (phase_ == TunnelPhase::ConnectingCstp) {
            timing_.timer.end(ConnectTiming::CSTP_CONNECT);
            timing_.timer.start(ConnectTiming::NETWORK_CONFIG);
            transition_to(TunnelPhase::ApplyingNetworkConfig);

            // When the native engine is active, apply the tunnel config
            // now and auto-advance the rest of the flow.
            if (runner_.is_running()) {
                apply_tunnel_config_and_advance();
            }
        }
    }

    void apply_tunnel_config_and_advance() {
        log_tunnel_event("INFO", "network.config.applying", "Applying network config",
                         {{"session_id", session_id_.value}});
        try {
            exv::helper::ApplyTunnelConfigRequest cfg_req;
            cfg_req.config.session_id        = session_id_;

            // Use the real IP address assigned by the VPN gateway, falling
            // back to a safe default only when the engine hasn't reported one.
            auto engine_status = runner_.status();
            std::string ip = engine_status.internal_ip;
            if (ip.empty()) {
                ip = "10.0.0.2/24";  // safe fallback
            } else if (ip.find('/') == std::string::npos) {
                // The engine reports a bare IP; append a sensible prefix.
                ip += "/24";
            }
            cfg_req.config.interface_address = ip;
            cfg_req.config.enable_kill_switch = false;

            auto cfg_resp = helper_->apply_tunnel_config(cfg_req);
            if (!cfg_resp.success) {
                timing_.timer.end(ConnectTiming::NETWORK_CONFIG);
                set_error(CoreErrorMapper::from_helper_error(
                    "apply_config_failed", cfg_resp.error_message));
                transition_to(TunnelPhase::Failed);
                return;
            }
        } catch (const std::exception& e) {
            timing_.timer.end(ConnectTiming::NETWORK_CONFIG);
            set_error(CoreErrorMapper::from_helper_error(
                "apply_config_failed", e.what()));
            transition_to(TunnelPhase::Failed);
            return;
        }
        timing_.timer.end(ConnectTiming::NETWORK_CONFIG);

        // Transition through OpeningPacketDevice and fire
        // NetworkConfigApplied so the state machine advances.
        timing_.timer.start(ConnectTiming::PACKET_DEVICE);
        transition_to(TunnelPhase::OpeningPacketDevice);

        // The engine's packet-loop thread handles the packet device.
        // When it starts, it will fire PacketLoopStarted.
    }

    void on_network_config_applied() {
        if (phase_ == TunnelPhase::ApplyingNetworkConfig) {
            timing_.timer.end(ConnectTiming::NETWORK_CONFIG);
            timing_.timer.start(ConnectTiming::PACKET_DEVICE);
            transition_to(TunnelPhase::OpeningPacketDevice);
        }
    }

    void on_packet_loop_started() {
        if (phase_ == TunnelPhase::OpeningPacketDevice) {
            log_tunnel_event("INFO", "packet.loop.started", "Packet loop started",
                             {{"session_id", session_id_.value}});
            timing_.timer.end(ConnectTiming::PACKET_DEVICE);
            transition_to(TunnelPhase::Connected);
            log_tunnel_event("INFO", "connect.connected", "Tunnel connected",
                             {{"session_id", session_id_.value}});
            reconnect_attempts_ = 0;
            reconnect_policy_.reset();
            start_heartbeat();
        }
    }

    void on_transport_closed() {
        if (phase_ != TunnelPhase::Connected
            && phase_ != TunnelPhase::Reconnecting) {
            return;
        }

        stop_heartbeat();

        auto err = CoreErrorMapper::from_transport_error(-1, "transport");

        if (intent_.auto_reconnect) {
            attempt_reconnect(err);
        } else {
            set_error(err);
            transition_to(TunnelPhase::Failed);
        }
    }

    void on_packet_device_failed() {
        if (phase_ != TunnelPhase::OpeningPacketDevice
            && phase_ != TunnelPhase::Connected) {
            return;
        }

        stop_heartbeat();

        auto err = CoreErrorMapper::from_platform_error(
            "packet", -1, "packet_device");
        err.code = "packet_device_failed";
        err.recoverable = false;

        set_error(err);
        transition_to(TunnelPhase::Failed);
    }

    void on_lease_expired() {
        if (phase_ != TunnelPhase::Connected) {
            return;
        }

        auto err = CoreErrorMapper::from_transport_error(-1, "lease_expired");
        err.code = "lease_expired";
        err.recoverable = true;

        if (intent_.auto_reconnect) {
            attempt_reconnect(err);
        } else {
            set_error(err);
            transition_to(TunnelPhase::Failed);
        }
    }

    void on_reconnect_timer_fired() {
        if (phase_ != TunnelPhase::Reconnecting) {
            return;
        }

        // Retry the connect flow starting from Authenticating.
        // In a real implementation this would re-initiate the auth/CSTP
        // negotiation.  For the simplified version we just move to
        // Authenticating and let subsequent events drive the flow.
        timing_.timer.start(ConnectTiming::AUTH);
        transition_to(TunnelPhase::Authenticating);
    }

    void on_helper_lost() {
        // Nothing to lose if we're already idle or failed.
        if (phase_ == TunnelPhase::Idle || phase_ == TunnelPhase::Failed) {
            return;
        }

        stop_heartbeat();

        auto err = CoreErrorMapper::from_helper_error(
            "helper_lost", "Helper process disconnected unexpectedly");

        if (intent_.auto_reconnect) {
            // Attempt to re-establish the helper connection.
            try {
                if (helper_ && helper_->connect()) {
                    attempt_reconnect(err);
                    return;
                }
            } catch (...) {
                // Helper reconnection failed — fall through.
            }
        }

        set_error(err);
        transition_to(TunnelPhase::Failed);
    }

    void on_cleanup_succeeded() {
        transition_to(TunnelPhase::Idle);
    }

    void on_cleanup_failed() {
        // Best-effort: still move to Idle.
        transition_to(TunnelPhase::Idle);
    }
