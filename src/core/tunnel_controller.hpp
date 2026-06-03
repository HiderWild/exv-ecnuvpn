#pragma once
#include <memory>
#include <functional>
#include "tunnel_intent.hpp"
#include "tunnel_state.hpp"
#include "tunnel_events.hpp"
#include "reconnect_policy.hpp"

// Forward declarations — the real interfaces live in helper / platform.
namespace exv::helper { class HelperClient; }
namespace exv::platform { class PlatformNetworkOps; }

namespace exv::core {

class TunnelController {
public:
    TunnelController(
        std::shared_ptr<exv::helper::HelperClient> helper,
        std::shared_ptr<exv::platform::PlatformNetworkOps> net_ops,
        ReconnectConfig reconnect_config = {}
    );
    ~TunnelController();

    // User intent interface
    void connect(UserIntent intent);
    void disconnect(DisconnectReason reason = DisconnectReason::UserRequested);
    void set_auto_reconnect(bool enabled);

    // Status
    TunnelStatusSnapshot status() const;
    TunnelPhase phase() const;

    // Event processing (called by engine/platform callbacks)
    void on_event(TunnelEvent event);

    // Status change callback
    using StatusCallback = std::function<void(const TunnelStatusSnapshot&)>;
    void set_status_callback(StatusCallback cb);

private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

} // namespace exv::core
