#pragma once

#include "core/config/config.hpp"
#include "engine_event_bridge.hpp"
#include "vpn_engine/engine.hpp"
#include "vpn_engine/native_engine.hpp"
#include "tunnel_events.hpp"
#include "tunnel_state.hpp"

#include <atomic>
#include <condition_variable>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <vector>

namespace exv::core {

/// CoreSessionRunner bridges the NativeVpnEngineSession (ecnuvpn::vpn_engine)
/// into TunnelController's event-driven model.
///
/// Responsibilities:
///   - Create and own a NativeVpnEngineSession on start()
///   - Capture VpnEngineEvents via EngineEventBridge and translate them into
///     TunnelEvents, invoking the registered EventCallback
///   - Monitor the session lifecycle on a background thread
///   - Provide thread-safe stop / status queries
class CoreSessionRunner {
public:
    using EventCallback = std::function<void(TunnelEvent)>;
    using NativeDependenciesFactory =
        std::function<ecnuvpn::vpn_engine::NativeVpnEngineDependencies()>;
    using NetworkConfigCallback =
        std::function<ecnuvpn::vpn_engine::ValidationResult(
            const ecnuvpn::vpn_engine::TunnelMetadata&,
            ecnuvpn::vpn_engine::DeviceConfig*)>;

    struct PendingAuthInteraction {
        std::string id;
        std::string kind;
        std::string label;
        std::string input_type;
        std::vector<std::string> options;
    };

    CoreSessionRunner();
    explicit CoreSessionRunner(NativeDependenciesFactory deps_factory);
    ~CoreSessionRunner();

    CoreSessionRunner(const CoreSessionRunner&) = delete;
    CoreSessionRunner& operator=(const CoreSessionRunner&) = delete;

    /// Start the VPN session.
    /// Converts the ecnuvpn::Config into VpnEngineConfig and creates a
    /// NativeVpnEngineSession.  The session runs its own packet-loop thread.
    /// Returns false if start() fails immediately (e.g. validation error).
    bool start(const ecnuvpn::Config& cfg, const std::string& password);

    /// Stop the running session (graceful, then join monitoring thread).
    void stop();

    /// True while the session is active (between successful start() and
    /// session finish or stop()).
    bool is_running() const;

    /// Current VpnEngineStatus snapshot (thread-safe).
    ecnuvpn::vpn_engine::VpnEngineStatus status() const;
    std::optional<PendingAuthInteraction> pending_auth_interaction() const;
    bool provide_auth_interaction_response(const std::string& id,
                                           const std::string& value);

    /// Register the callback that receives translated TunnelEvents.
    void set_event_callback(EventCallback cb);
    void set_network_config_callback(NetworkConfigCallback cb);

private:
    struct AuthInteractionResponseState {
        std::string id;
        std::string value;
    };

    ecnuvpn::vpn_engine::protocol::AuthInteractionResponse
    handle_auth_interaction(
        const ecnuvpn::vpn_engine::protocol::AuthInteractionRequest& request);

    mutable std::mutex mu_;
    std::condition_variable auth_interaction_cv_;
    std::unique_ptr<ecnuvpn::vpn_engine::NativeVpnEngineSession> session_;
    std::unique_ptr<EngineEventBridge> bridge_;
    std::thread monitor_thread_;
    EventCallback event_callback_;
    NetworkConfigCallback network_config_callback_;
    NativeDependenciesFactory deps_factory_;
    std::optional<PendingAuthInteraction> pending_auth_interaction_;
    std::optional<AuthInteractionResponseState> auth_interaction_response_;
    std::atomic<bool> running_{false};
    ecnuvpn::vpn_engine::VpnEngineStatus cached_status_;
};

} // namespace exv::core
