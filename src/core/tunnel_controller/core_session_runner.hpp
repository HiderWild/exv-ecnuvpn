#pragma once

#include "engine_event_bridge.hpp"
#include "vpn_engine/engine.hpp"
#include "vpn_engine/native_engine.hpp"
#include "tunnel_events.hpp"
#include "tunnel_state.hpp"

#include <atomic>
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

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

    /// Register the callback that receives translated TunnelEvents.
    void set_event_callback(EventCallback cb);
    void set_network_config_callback(NetworkConfigCallback cb);

private:
    mutable std::mutex mu_;
    std::unique_ptr<ecnuvpn::vpn_engine::NativeVpnEngineSession> session_;
    std::unique_ptr<EngineEventBridge> bridge_;
    std::thread monitor_thread_;
    EventCallback event_callback_;
    NetworkConfigCallback network_config_callback_;
    NativeDependenciesFactory deps_factory_;
    std::atomic<bool> running_{false};
    ecnuvpn::vpn_engine::VpnEngineStatus cached_status_;
};

} // namespace exv::core
