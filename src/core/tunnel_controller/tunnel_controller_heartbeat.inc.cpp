    // ================================================================
    // Heartbeat — keeps the helper session alive
    // ================================================================

    void start_heartbeat() {
        if (heartbeat_active_) return;
        if (!helper_ || !helper_->is_connected()) return;
        heartbeat_active_ = true;
        do_heartbeat();
    }

    void stop_heartbeat() {
        heartbeat_active_ = false;
        // No need to cancel scheduler — the callback checks heartbeat_active_.
    }

    void schedule_next_heartbeat() {
        if (!heartbeat_active_) return;
        scheduler_.schedule(kHeartbeatInterval, [this] {
            do_heartbeat();
        });
    }

    void do_heartbeat() {
        if (!heartbeat_active_) return;
        if (!helper_ || !helper_->is_connected()) {
            stop_heartbeat();
            return;
        }

        try {
            exv::helper::HeartbeatRequest req;
            req.session_id = session_id_;
            // Map current phase to a string the helper understands.
            switch (phase_) {
                case TunnelPhase::PreparingHelper: req.core_phase = "PreparingHelper"; break;
                case TunnelPhase::Connected:     req.core_phase = "Connected"; break;
                case TunnelPhase::Reconnecting:  req.core_phase = "Reconnecting"; break;
                case TunnelPhase::Authenticating: req.core_phase = "Authenticating"; break;
                case TunnelPhase::ConnectingCstp: req.core_phase = "ConnectingCstp"; break;
                case TunnelPhase::ApplyingNetworkConfig: req.core_phase = "ApplyingNetworkConfig"; break;
                case TunnelPhase::OpeningPacketDevice: req.core_phase = "OpeningPacketDevice"; break;
                default:                          req.core_phase = "Idle"; break;
            }

            auto resp = helper_->heartbeat(req);
            if (!resp.ok) {
                log_tunnel_event("WARN", "heartbeat.failed",
                                 "Heartbeat response not ok",
                                 {{"warning", resp.warning.value_or("")}});
            }
        } catch (const std::exception& e) {
            log_tunnel_event("ERROR", "heartbeat.error",
                             "Heartbeat send failed",
                             {{"error", e.what()}});
            stop_heartbeat();
            // Don't fire HelperLost here — let the next actual helper
            // operation detect the disconnection naturally.
            return;
        }

        // Schedule the next heartbeat.
        schedule_next_heartbeat();
    }
