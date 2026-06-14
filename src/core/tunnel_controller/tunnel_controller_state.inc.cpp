    // ================================================================
    // State transition helpers
    // ================================================================

    void transition_to(TunnelPhase new_phase) {
        if (new_phase == TunnelPhase::Idle ||
            new_phase == TunnelPhase::Failed ||
            new_phase == TunnelPhase::CleaningUp) {
            stop_heartbeat();
        }
        phase_ = new_phase;
        update_snapshot();
        notify_status();
    }

    void update_snapshot() {
        snapshot_.phase            = phase_;
        snapshot_.desired_connected = intent_.desired_connected;
        snapshot_.auto_reconnect   = intent_.auto_reconnect;
        snapshot_.server           = intent_.profile_id.value;
        snapshot_.interface_name   = adapter_name_;

        // Helper status — in the real implementation this would query the
        // helper; here we derive it from the client connection state.
        if (helper_ && helper_->is_connected()) {
            snapshot_.helper_status = "connected";
        } else {
            snapshot_.helper_status = "unavailable";
        }
        snapshot_.helper_mode = "transient";

        // Network-ready is true once we have fully connected.
        snapshot_.network_ready = (phase_ == TunnelPhase::Connected);

        // Reconnect bookkeeping
        if (phase_ == TunnelPhase::Reconnecting) {
            ReconnectInfo ri;
            ri.attempt       = reconnect_attempts_;
            ri.next_retry_ms = static_cast<int>(reconnect_policy_.next_delay().count());
            snapshot_.reconnect = ri;
        } else {
            snapshot_.reconnect = std::nullopt;
        }
    }

    void notify_status() {
        if (status_callback_) {
            status_callback_(snapshot_);
        }
    }

    void set_error(const ErrorInfo& error) {
        snapshot_.last_error = error;
    }

    void clear_error() {
        snapshot_.last_error = std::nullopt;
    }
