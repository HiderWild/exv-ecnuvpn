#include "core/tunnel_controller/core_session_runner.hpp"
#include "core/tunnel_controller/native_engine_config_mapper.hpp"

namespace exv::core {

// =========================================================================
// CoreSessionRunner
// =========================================================================

CoreSessionRunner::CoreSessionRunner()
    : CoreSessionRunner([] {
          return ecnuvpn::vpn_engine::default_native_engine_dependencies();
      }) {}

CoreSessionRunner::CoreSessionRunner(NativeDependenciesFactory deps_factory)
    : deps_factory_(std::move(deps_factory)) {}

CoreSessionRunner::~CoreSessionRunner() {
    stop();
}

bool CoreSessionRunner::start(const ecnuvpn::Config& cfg,
                              const std::string& password) {
    std::unique_lock<std::mutex> lock(mu_);

    if (running_) return false;

    // Build VpnEngineConfig from the application Config at the core boundary.
    ecnuvpn::vpn_engine::VpnEngineConfig engine_config;
    auto config_result =
        make_native_engine_config(cfg, password, &engine_config);
    if (!config_result.ok) return false;
    // TunnelController owns helper/network reconnect, so the native engine must
    // report transport closure instead of reconnecting internally.
    engine_config.auto_reconnect = false;

    // Create the event bridge — it translates VpnEngineEvents to TunnelEvents
    // and invokes the callback.
    bridge_ = std::make_unique<EngineEventBridge>(
        [this](TunnelEvent te) {
            // The bridge callback is invoked from the engine's background
            // thread.  Copy the callback under lock, then invoke outside
            // to avoid holding mu_ during user code.
            EventCallback cb;
            {
                std::lock_guard<std::mutex> lk(mu_);
                if (session_) {
                    cached_status_ = session_->status();
                }
                cb = event_callback_;
            }
            if (cb) cb(te);
        });

    // Create NativeVpnEngineSession with default dependencies and the bridge
    // as the event sink.
    auto deps = deps_factory_ ? deps_factory_()
                              : ecnuvpn::vpn_engine::default_native_engine_dependencies();
    deps.event_sink = bridge_.get();
    deps.network_configurator = network_config_callback_;

    session_ = std::make_unique<ecnuvpn::vpn_engine::NativeVpnEngineSession>(
        engine_config, deps);

    // Unlock before calling session_->start() because it runs auth + CSTP
    // synchronously and emits events through the bridge callback, which
    // needs to acquire mu_ to read session_ / event_callback_.  Holding mu_
    // across the synchronous start() would deadlock on the same thread.
    lock.unlock();

    // Validate config and start the session (auth + CSTP negotiation happen
    // synchronously during start(); the packet loop runs on a background
    // thread inside the session).
    auto validation = session_->start();

    lock.lock();
    if (!validation.ok) {
        cached_status_ = session_ ? session_->status()
                                  : ecnuvpn::vpn_engine::VpnEngineStatus{};

        TunnelEvent te;
        if (validation.code.rfind("packet", 0) == 0 ||
            validation.code.rfind("native_packet", 0) == 0) {
            te.type = TunnelEventType::PacketDeviceFailed;
        } else if (validation.code == "auth_failed" ||
                   validation.code == "unsupported_auth_flow") {
            te.type = TunnelEventType::AuthFailed;
        } else {
            te.type = TunnelEventType::TransportClosed;
        }

        // Fire the failure event outside the lock.
        EventCallback cb = event_callback_;
        lock.unlock();

        if (cb) {
            cb(te);
        }

        lock.lock();
        session_.reset();
        bridge_.reset();
        return false;
    }

    cached_status_ = session_->status();
    running_ = true;

    // Spawn a monitoring thread that waits for the session to finish and
    // then fires a TunnelEvent to notify the controller.
    monitor_thread_ = std::thread([this]() {
        while (true) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));

            // Check session status under lock.
            EventCallback cb;
            bool session_finished = false;
            bool has_error = false;
            TunnelEvent error_event;

            {
                std::lock_guard<std::mutex> lk(mu_);
                if (!session_) break;

                auto st = session_->status();
                cached_status_ = st;

                if (!st.running) {
                    running_ = false;
                    session_finished = true;

                    if (!st.error_code.empty()) {
                        has_error = true;
                        error_event.type = TunnelEventType::TransportClosed;
                    }
                    cb = event_callback_;
                }
            }

            if (session_finished) {
                // Fire the event outside the lock.
                if (has_error && cb) {
                    cb(error_event);
                }
                break;
            }
        }
    });

    return true;
}

void CoreSessionRunner::stop() {
    std::unique_ptr<ecnuvpn::vpn_engine::NativeVpnEngineSession> session;

    {
        std::lock_guard<std::mutex> lock(mu_);
        session = std::move(session_);
        running_ = false;
    }

    if (session) {
        session->stop();
    }

    if (monitor_thread_.joinable()) {
        monitor_thread_.join();
    }

    std::lock_guard<std::mutex> lock(mu_);
    bridge_.reset();
    cached_status_ = {};
}

bool CoreSessionRunner::is_running() const {
    std::lock_guard<std::mutex> lock(mu_);
    return running_;
}

ecnuvpn::vpn_engine::VpnEngineStatus CoreSessionRunner::status() const {
    std::lock_guard<std::mutex> lock(mu_);
    if (session_) {
        return session_->status();
    }
    return cached_status_;
}

void CoreSessionRunner::set_event_callback(EventCallback cb) {
    std::lock_guard<std::mutex> lock(mu_);
    event_callback_ = std::move(cb);
}

void CoreSessionRunner::set_network_config_callback(NetworkConfigCallback cb) {
    std::lock_guard<std::mutex> lock(mu_);
    network_config_callback_ = std::move(cb);
}

} // namespace exv::core
