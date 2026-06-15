#pragma once
#include <functional>
#include <memory>
#include <string>
#include "tunnel_intent.hpp"
#include "tunnel_state.hpp"
#include "tunnel_events.hpp"
#include "reconnect_policy.hpp"

// Forward declarations — the real interfaces live in helper / platform.
namespace exv::helper { class HelperClient; }
namespace exv::platform { class PlatformNetworkOps; }
namespace ecnuvpn { struct Config; }

namespace exv::core {

class TunnelControllerTestAccess;

class TunnelController {
public:
    TunnelController(
        std::shared_ptr<exv::helper::HelperClient> helper,
        std::shared_ptr<exv::platform::PlatformNetworkOps> net_ops,
        ReconnectConfig reconnect_config = {}
    );
    ~TunnelController();

    /// Provide the VPN config and plaintext password used by the native
    /// engine.  Must be called before connect() when using the real engine.
    void set_vpn_config(const ecnuvpn::Config& cfg,
                        const std::string& plaintext_password);

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
    friend class TunnelControllerTestAccess;

    struct Impl;
    std::unique_ptr<Impl> impl_;
};

} // namespace exv::core
