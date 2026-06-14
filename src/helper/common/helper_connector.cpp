#include "helper_connector.hpp"
#include "helper_client.hpp"
#include "helper_messages.hpp"
#include "helper_error.hpp"
#include "pipe_helper_client.hpp"
#include "logger.hpp"

#include <stdexcept>

namespace exv::helper {

// ---------------------------------------------------------------------------
// PlatformHelperConnector -- production connector using named pipes / Unix sockets
// ---------------------------------------------------------------------------

class PlatformHelperConnector : public HelperConnector {
public:
    std::unique_ptr<HelperClient> connect(const HelperConnectorConfig& config) override {
        PipeClientConfig pc;
        pc.pipe_path = resolve_endpoint(config);
        pc.connect_timeout_ms = config.connect_timeout_ms;

        ecnuvpn::logger::info("Helper connector: Attempting connection - endpoint=" + 
                              pc.pipe_path + " timeout_ms=" + 
                              std::to_string(pc.connect_timeout_ms));

        auto client = std::make_unique<PipeHelperClient>(pc);
        if (!client->connect()) {
            ecnuvpn::logger::error("Helper connector: Connection failed - endpoint=" + pc.pipe_path);
            return nullptr;
        }
        
        ecnuvpn::logger::info("Helper connector: Connected successfully - endpoint=" + pc.pipe_path);
        return client;
    }

    bool is_helper_available() const override {
        // Best-effort: try connecting to the default endpoint.
        // Returns false if the daemon is not running.
        PipeClientConfig pc;
        pc.pipe_path = default_endpoint();
        pc.connect_timeout_ms = 500;  // quick probe
        PipeHelperClient probe(pc);
        return probe.connect();
    }

private:
    /// Determine the pipe / socket endpoint from the connector config.
    /// Priority: 1) explicit pipe_endpoint, 2) helper_executable_path if it
    /// looks like a pipe/socket, 3) platform default endpoint.
    static std::string resolve_endpoint(const HelperConnectorConfig& config) {
        // 1) Explicit pipe endpoint takes highest priority.
        if (!config.pipe_endpoint.empty()) {
            return config.pipe_endpoint;
        }

        // 2) helper_executable_path may carry a pipe/socket endpoint
        //    (legacy callers that don't set pipe_endpoint separately).
        if (!config.helper_executable_path.empty()) {
            const auto& p = config.helper_executable_path;
#ifdef _WIN32
            // On Windows, named pipes start with \\.\pipe\ or \\?\pipe\
            if (p.find("\\\\.\\pipe\\") == 0 || p.find("\\\\?\\pipe\\") == 0)
                return p;
#else
            // On POSIX, absolute paths are socket paths
            if (!p.empty() && p[0] == '/')
                return p;
#endif
        }
        return default_endpoint();
    }

    /// Platform-specific default endpoint for the Helper daemon.
    static std::string default_endpoint() {
#ifdef _WIN32
        return "\\\\.\\pipe\\exv-helper";
#elif defined(__APPLE__)
        return "/var/run/exv-helper.sock";
#else
        return "/var/run/exv-helper.sock";
#endif
    }
};

// ---------------------------------------------------------------------------
// StubHelperClient -- kept for unit testing only (no real IPC).
// ---------------------------------------------------------------------------

class StubHelperClient : public HelperClient {
public:
    bool connect() override {
        connected_ = true;
        return true;
    }

    void disconnect() override {
        connected_ = false;
    }

    bool is_connected() const override {
        return connected_;
    }

    HelloResponse hello(const HelloRequest& /*req*/) override {
        HelloResponse resp;
        resp.server_version = PROTOCOL_VERSION;
        resp.capabilities = {"tunnel_device_create", "route_apply", "dns_apply", "route_cleanup"};
        resp.mode = HelperMode::Transient;
        return resp;
    }

    StartSessionResponse start_session(const StartSessionRequest& /*req*/) override {
        StartSessionResponse resp;
        resp.session_id.value = "stub-session-" + std::to_string(++session_counter_);
        return resp;
    }

    PrepareTunnelDeviceResponse prepare_tunnel_device(const PrepareTunnelDeviceRequest& req) override {
        PrepareTunnelDeviceResponse resp;
        resp.device_path = "//./StubTun/" + req.session_id.value;
        resp.mtu = 1400;
        return resp;
    }

    ApplyTunnelConfigResponse apply_tunnel_config(const ApplyTunnelConfigRequest& /*req*/) override {
        ApplyTunnelConfigResponse resp;
        resp.success = true;
        return resp;
    }

    HeartbeatResponse heartbeat(const HeartbeatRequest& /*req*/) override {
        HeartbeatResponse resp;
        resp.ok = true;
        return resp;
    }

    CleanupResponse cleanup(const CleanupRequest& /*req*/) override {
        CleanupResponse resp;
        resp.success = true;
        return resp;
    }

    GetSnapshotResponse get_snapshot(const GetSnapshotRequest& /*req*/) override {
        GetSnapshotResponse resp;
        return resp;
    }

    EndSessionResponse end_session(const EndSessionRequest& /*req*/) override {
        EndSessionResponse resp;
        resp.success = true;
        return resp;
    }

    void set_disconnect_callback(DisconnectCallback cb) override {
        disconnect_cb_ = std::move(cb);
    }

private:
    bool connected_ = false;
    int session_counter_ = 0;
    DisconnectCallback disconnect_cb_;
};

// ---------------------------------------------------------------------------
// StubHelperConnector -- for unit testing only.
// ---------------------------------------------------------------------------

class StubHelperConnector : public HelperConnector {
public:
    std::unique_ptr<HelperClient> connect(const HelperConnectorConfig& /*config*/) override {
        auto client = std::make_unique<StubHelperClient>();
        client->connect();
        return client;
    }

    bool is_helper_available() const override {
        return true;
    }
};

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

std::unique_ptr<HelperConnector> HelperConnector::create() {
    return std::make_unique<PlatformHelperConnector>();
}

std::unique_ptr<HelperConnector> HelperConnector::create_stub() {
    return std::make_unique<StubHelperConnector>();
}

} // namespace exv::helper
