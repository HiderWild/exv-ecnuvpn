#include "tunnel_controller.hpp"
#include "core_error_mapper.hpp"
#include "timing.hpp"
#include "../helper_common/helper_client.hpp"
#include "../platform/common/platform_network_ops.hpp"

#include <stdexcept>
#include <string>

namespace exv::core {

// =========================================================================
// Impl — holds all state and drives the state machine
// =========================================================================

struct TunnelController::Impl {

    // --- Dependencies (injected) ---
    std::shared_ptr<exv::helper::HelperClient>  helper_;
    std::shared_ptr<exv::platform::PlatformNetworkOps> net_ops_;

    // --- State machine ---
    ReconnectPolicy      reconnect_policy_;
    TunnelPhase          phase_   = TunnelPhase::Idle;
    UserIntent           intent_;
    TunnelStatusSnapshot snapshot_;
    StatusCallback       status_callback_;

    // --- Timing ---
    ConnectTiming timing_;

    // --- Session bookkeeping ---
    exv::helper::SessionId session_id_;
    std::string            adapter_name_{"ECNU-VPN"};

    // --- Reconnect tracking ---
    int reconnect_attempts_ = 0;

    // ================================================================
    // State transition helpers
    // ================================================================

    void transition_to(TunnelPhase new_phase) {
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

    // ================================================================
    // Guards
    // ================================================================

    bool can_start_connect() const {
        return phase_ == TunnelPhase::Idle
            || phase_ == TunnelPhase::Failed;
    }

    bool can_disconnect() const {
        return phase_ != TunnelPhase::Idle
            && phase_ != TunnelPhase::Disconnecting
            && phase_ != TunnelPhase::CleaningUp
            && phase_ != TunnelPhase::Failed;
    }

    // ================================================================
    // Connect flow (simplified synchronous implementation)
    //
    // 1.  Store intent
    // 2.  PreparingHelper       — start_session()
    // 3.  Authenticating        — (simulated)
    // 4.  ConnectingCstp        — (simulated)
    // 5.  ApplyingNetworkConfig — apply_tunnel_config()
    // 6.  OpeningPacketDevice   — prepare_tunnel_device()
    // 7.  Connected
    // ================================================================

    void do_connect() {
        // Step 1 — bookkeeping
        intent_.desired_connected = true;
        clear_error();
        timing_.timer.reset();
        reconnect_attempts_ = 0;

        // Step 2 — PreparingHelper: start helper session
        timing_.timer.start(ConnectTiming::HELPER_PREPARE);
        transition_to(TunnelPhase::PreparingHelper);

        try {
            exv::helper::StartSessionRequest req;
            req.profile_id.value = intent_.profile_id.value;
            req.mode = exv::helper::HelperMode::Transient;

            auto resp = helper_->start_session(req);
            session_id_ = resp.session_id;
        } catch (const std::exception& e) {
            timing_.timer.end(ConnectTiming::HELPER_PREPARE);
            set_error(CoreErrorMapper::from_helper_error(
                "start_session_failed", e.what()));
            transition_to(TunnelPhase::Failed);
            return;
        }
        timing_.timer.end(ConnectTiming::HELPER_PREPARE);

        // Step 3 — Authenticating (simulated: real auth not wired yet)
        timing_.timer.start(ConnectTiming::AUTH);
        transition_to(TunnelPhase::Authenticating);
        timing_.timer.end(ConnectTiming::AUTH);

        // Step 4 — ConnectingCstp (simulated)
        timing_.timer.start(ConnectTiming::CSTP_CONNECT);
        transition_to(TunnelPhase::ConnectingCstp);
        timing_.timer.end(ConnectTiming::CSTP_CONNECT);

        // Step 5 — ApplyingNetworkConfig: ask helper to push routes / DNS
        timing_.timer.start(ConnectTiming::NETWORK_CONFIG);
        transition_to(TunnelPhase::ApplyingNetworkConfig);

        try {
            exv::helper::ApplyTunnelConfigRequest cfg_req;
            cfg_req.config.session_id        = session_id_;
            cfg_req.config.interface_address = "10.0.0.2/24";   // placeholder
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

        // Step 6 — OpeningPacketDevice: create the tunnel adapter
        timing_.timer.start(ConnectTiming::PACKET_DEVICE);
        transition_to(TunnelPhase::OpeningPacketDevice);

        try {
            auto device = net_ops_->prepare_tunnel_device(adapter_name_);
            if (device.path.empty()) {
                timing_.timer.end(ConnectTiming::PACKET_DEVICE);
                auto err = CoreErrorMapper::from_platform_error(
                    "packet", -1, "prepare_tunnel_device");
                err.message = "Tunnel device returned empty path";
                set_error(err);
                transition_to(TunnelPhase::Failed);
                return;
            }
        } catch (const std::exception& e) {
            timing_.timer.end(ConnectTiming::PACKET_DEVICE);
            auto err = CoreErrorMapper::from_platform_error(
                "packet", -1, "prepare_tunnel_device");
            err.message = std::string("prepare_tunnel_device: ") + e.what();
            set_error(err);
            transition_to(TunnelPhase::Failed);
            return;
        }
        timing_.timer.end(ConnectTiming::PACKET_DEVICE);

        // Step 7 — Connected
        transition_to(TunnelPhase::Connected);
        reconnect_policy_.reset();
    }

    // ================================================================
    // Disconnect flow
    // ================================================================

    void do_disconnect(DisconnectReason reason) {
        intent_.desired_connected    = false;
        intent_.user_disconnect_reason = reason;

        transition_to(TunnelPhase::Disconnecting);
        do_cleanup();
    }

    void do_cleanup() {
        transition_to(TunnelPhase::CleaningUp);

        try {
            exv::helper::CleanupRequest req;
            req.session_id = session_id_;
            req.policy.remove_routes       = true;
            req.policy.remove_dns          = true;
            req.policy.remove_adapter      = false;   // keep for reconnect
            req.policy.remove_firewall_rules = true;

            auto resp = helper_->cleanup(req);

            if (!resp.success) {
                // Log per-resource errors but still move to Idle (best effort)
            }
        } catch (const std::exception&) {
            // Cleanup threw — nothing we can do; move to Idle anyway.
        }

        transition_to(TunnelPhase::Idle);
    }

    // ================================================================
    // Reconnect decision
    // ================================================================

    void attempt_reconnect(const ErrorInfo& error) {
        auto decision = reconnect_policy_.decide(
            error, intent_, phase_, reconnect_attempts_);

        if (decision.should_retry) {
            ++reconnect_attempts_;
            transition_to(TunnelPhase::Reconnecting);
            // In a real implementation a timer would be scheduled using
            // decision.delay.  The ReconnectTimerFired event would then
            // trigger the actual retry.
        } else {
            set_error(error);
            transition_to(TunnelPhase::Failed);
        }
    }

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
            set_error(CoreErrorMapper::from_auth_error(-1, "Authentication failed"));
            transition_to(TunnelPhase::Failed);
        }
    }

    void on_cstp_connected() {
        if (phase_ == TunnelPhase::ConnectingCstp) {
            timing_.timer.end(ConnectTiming::CSTP_CONNECT);
            timing_.timer.start(ConnectTiming::NETWORK_CONFIG);
            transition_to(TunnelPhase::ApplyingNetworkConfig);
        }
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
            timing_.timer.end(ConnectTiming::PACKET_DEVICE);
            transition_to(TunnelPhase::Connected);
            reconnect_attempts_ = 0;
            reconnect_policy_.reset();
        }
    }

    void on_transport_closed() {
        if (phase_ != TunnelPhase::Connected
            && phase_ != TunnelPhase::Reconnecting) {
            return;
        }

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
};

// =========================================================================
// TunnelController — pimpl delegation
// =========================================================================

TunnelController::TunnelController(
    std::shared_ptr<exv::helper::HelperClient> helper,
    std::shared_ptr<exv::platform::PlatformNetworkOps> net_ops,
    ReconnectConfig reconnect_config)
    : impl_(std::make_unique<Impl>())
{
    impl_->helper_          = std::move(helper);
    impl_->net_ops_         = std::move(net_ops);
    impl_->reconnect_policy_ = ReconnectPolicy(reconnect_config);
}

TunnelController::~TunnelController() = default;

// ------------------------------------------------------------------
// User intent interface
// ------------------------------------------------------------------

void TunnelController::connect(UserIntent intent) {
    if (!impl_->can_start_connect()) {
        return;   // reject — already connecting / connected / disconnecting
    }
    impl_->intent_ = std::move(intent);
    impl_->do_connect();
}

void TunnelController::disconnect(DisconnectReason reason) {
    if (!impl_->can_disconnect()) {
        return;   // nothing to disconnect from
    }
    impl_->do_disconnect(reason);
}

void TunnelController::set_auto_reconnect(bool enabled) {
    impl_->intent_.auto_reconnect = enabled;
    impl_->update_snapshot();
    impl_->notify_status();
}

// ------------------------------------------------------------------
// Status
// ------------------------------------------------------------------

TunnelStatusSnapshot TunnelController::status() const {
    return impl_->snapshot_;
}

TunnelPhase TunnelController::phase() const {
    return impl_->phase_;
}

// ------------------------------------------------------------------
// Event processing
// ------------------------------------------------------------------

void TunnelController::on_event(TunnelEvent event) {
    switch (event.type) {

    // --- Connect-flow events ---
    case TunnelEventType::HelperReady:
        impl_->on_helper_ready();
        break;
    case TunnelEventType::AuthSucceeded:
        impl_->on_auth_succeeded();
        break;
    case TunnelEventType::AuthFailed:
        impl_->on_auth_failed();
        break;
    case TunnelEventType::CstpConnected:
        impl_->on_cstp_connected();
        break;
    case TunnelEventType::NetworkConfigApplied:
        impl_->on_network_config_applied();
        break;
    case TunnelEventType::PacketLoopStarted:
        impl_->on_packet_loop_started();
        break;

    // --- Failure / disconnect events ---
    case TunnelEventType::TransportClosed:
        impl_->on_transport_closed();
        break;
    case TunnelEventType::PacketDeviceFailed:
        impl_->on_packet_device_failed();
        break;
    case TunnelEventType::LeaseExpired:
        impl_->on_lease_expired();
        break;

    // --- Reconnect ---
    case TunnelEventType::ReconnectTimerFired:
        impl_->on_reconnect_timer_fired();
        break;

    // --- Helper lifecycle ---
    case TunnelEventType::HelperLost:
        impl_->on_helper_lost();
        break;

    // --- Cleanup ---
    case TunnelEventType::CleanupSucceeded:
        impl_->on_cleanup_succeeded();
        break;
    case TunnelEventType::CleanupFailed:
        impl_->on_cleanup_failed();
        break;

    // --- User intent events (handled by connect/disconnect/set_auto_reconnect) ---
    case TunnelEventType::UserConnect:
    case TunnelEventType::UserDisconnect:
    case TunnelEventType::SetAutoReconnect:
        // These are dispatched through the dedicated methods, not on_event.
        break;
    }
}

// ------------------------------------------------------------------
// Callback
// ------------------------------------------------------------------

void TunnelController::set_status_callback(StatusCallback cb) {
    impl_->status_callback_ = std::move(cb);
}

} // namespace exv::core
