#include "helper_connector.hpp"
#include "helper_client.hpp"
#include "helper_messages.hpp"
#include "helper_error.hpp"

namespace exv::helper {

// ---------------------------------------------------------------------------
// StubHelperClient -- temporary stub for Phase 2 testing.
// Real implementation will use named pipes / unix sockets in Phase 3.
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
// StubHelperConnector -- platform factory returns a stub for now.
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
    return std::make_unique<StubHelperConnector>();
}

} // namespace exv::helper
