#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/tunnel_controller/core_error_mapper.hpp"
#include "core/tunnel_controller/core_session_runner.hpp"
#include "core/tunnel_controller/timing.hpp"
#include "core/tunnel_controller/timer_scheduler.hpp"
#include "helper/common/helper_client.hpp"
#include "platform/common/platform_network_ops.hpp"
#include "helper/platform/helper_delegating_network_ops.hpp"
#include "core/config/config.hpp"

#include <stdexcept>
#include "logger.hpp"
#include <string>

namespace exv::core {
namespace {

void log_tunnel_event(const std::string& level,
                      const std::string& code,
                      const std::string& message,
                      const std::vector<std::pair<std::string, std::string>>& fields = {}) {
    ecnuvpn::logger::event(level, "tunnel", code, message, fields);
}

std::shared_ptr<exv::platform::HelperDelegatingPlatformNetworkOps>
as_helper_delegating_ops(const std::shared_ptr<exv::platform::PlatformNetworkOps>& ops) {
    return std::dynamic_pointer_cast<exv::platform::HelperDelegatingPlatformNetworkOps>(ops);
}

} // namespace

// =========================================================================
// Impl — holds all state and drives the state machine
// =========================================================================

struct TunnelController::Impl {
    Impl() = default;
    explicit Impl(CoreSessionRunner::NativeDependenciesFactory deps_factory)
        : runner_(std::move(deps_factory)) {}

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
    bool                   network_config_applied_ = false;

    // --- Reconnect tracking ---
    int reconnect_attempts_ = 0;

    // --- Timer scheduler (replaces fixed 2s supervisor retry loop) ---
    TimerScheduler scheduler_;

    // --- Heartbeat ---
    bool heartbeat_active_ = false;
    static constexpr auto kHeartbeatInterval = std::chrono::seconds(10);

    // --- Native engine session runner ---
    CoreSessionRunner runner_;

    // --- VPN config for native engine ---
    ecnuvpn::Config vpn_cfg_;
    std::string     vpn_password_;


#include "core/tunnel_controller/tunnel_controller_state.inc.cpp"
#include "core/tunnel_controller/tunnel_controller_heartbeat.inc.cpp"
#include "core/tunnel_controller/tunnel_controller_connect.inc.cpp"
#include "core/tunnel_controller/tunnel_controller_disconnect.inc.cpp"
#include "core/tunnel_controller/tunnel_controller_reconnect.inc.cpp"
#include "core/tunnel_controller/tunnel_controller_events.inc.cpp"
};

#include "core/tunnel_controller/tunnel_controller_facade.inc.cpp"

} // namespace exv::core
