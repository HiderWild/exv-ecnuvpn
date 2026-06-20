#include "core/tunnel_controller/tunnel_controller_impl.hpp"

namespace exv::core {

// ================================================================
// State transition helpers
// ================================================================

void TunnelController::Impl::transition_to(TunnelPhase new_phase) {
        if (new_phase == TunnelPhase::Idle ||
            new_phase == TunnelPhase::Failed ||
            new_phase == TunnelPhase::CleaningUp) {
            stop_heartbeat();
        }
        phase_ = new_phase;
        update_snapshot();
        notify_status();
    }

void TunnelController::Impl::update_snapshot() {
        snapshot_.phase            = phase_;
        snapshot_.desired_connected = intent_.desired_connected;
        snapshot_.auto_reconnect   = intent_.auto_reconnect;
        snapshot_.server           = intent_.profile_id.value;
        auto engine_status = runner_.status();
        snapshot_.interface_name =
            engine_status.interface_name.empty()
                ? adapter_name_
                : engine_status.interface_name;
        snapshot_.internal_ip =
            engine_status.internal_ip.empty()
                ? assigned_internal_ip_
                : engine_status.internal_ip;

        if (!helper_status_override_.empty()) {
            snapshot_.helper_status = helper_status_override_;
        } else if (helper_ && (helper_->is_connected() || helper_connected_seen_)) {
            snapshot_.helper_status = "connected";
        } else {
            snapshot_.helper_status = "unavailable";
        }
        snapshot_.helper_mode = helper_mode_;
        snapshot_.helper_endpoint = helper_endpoint_;
        snapshot_.core_lease_active = !core_lease_id_.empty();
        snapshot_.session_active = !session_id_.value.empty();

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

void TunnelController::Impl::notify_status() {
        if (status_callback_) {
            status_callback_(snapshot_);
        }
    }

void TunnelController::Impl::set_error(const ErrorInfo& error) {
        snapshot_.last_error = error;
    }

void TunnelController::Impl::clear_error() {
        snapshot_.last_error = std::nullopt;
    }

} // namespace exv::core
