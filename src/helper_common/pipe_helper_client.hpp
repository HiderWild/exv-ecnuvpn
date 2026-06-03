#pragma once
#include "helper_client.hpp"
#include "helper_messages.hpp"
#include <string>

namespace exv::helper {

/// Configuration for PipeHelperClient connection.
struct PipeClientConfig {
    std::string pipe_path;  // Named pipe path (e.g. \\.\pipe\exv-helper) or
                            // Unix socket path (e.g. /var/run/exv-helper.sock)
    int connect_timeout_ms = 5000;
};

/// Platform-specific HelperClient that communicates over Windows named pipes
/// or Unix domain sockets. Maintains a persistent connection for the V2
/// protocol session lifecycle (hello -> start_session -> ... -> end_session).
///
/// This is the production implementation used by the Core process to talk to
/// the Helper daemon (which runs with elevated privileges).
class PipeHelperClient : public HelperClient {
public:
    explicit PipeHelperClient(const PipeClientConfig& config);
    ~PipeHelperClient() override;

    // Connection management
    bool connect() override;
    void disconnect() override;
    bool is_connected() const override;

    // V2 protocol methods
    HelloResponse hello(const HelloRequest& req) override;
    StartSessionResponse start_session(const StartSessionRequest& req) override;
    PrepareTunnelDeviceResponse prepare_tunnel_device(const PrepareTunnelDeviceRequest& req) override;
    ApplyTunnelConfigResponse apply_tunnel_config(const ApplyTunnelConfigRequest& req) override;
    HeartbeatResponse heartbeat(const HeartbeatRequest& req) override;
    CleanupResponse cleanup(const CleanupRequest& req) override;
    GetSnapshotResponse get_snapshot(const GetSnapshotRequest& req) override;
    EndSessionResponse end_session(const EndSessionRequest& req) override;

    void set_disconnect_callback(DisconnectCallback cb) override;

private:
    /// Send a V2 envelope request and receive the response.
    /// Handles serialization, transport, and deserialization.
    HelperResponse send_request(HelperOp op, const nlohmann::json& payload);

    /// Low-level: write data to the transport.
    bool send_raw(const std::string& data);

    /// Low-level: read a newline-delimited message from the transport.
    std::string recv_raw();

    PipeClientConfig config_;
    bool connected_ = false;
    DisconnectCallback disconnect_cb_;

#ifdef _WIN32
    void* pipe_handle_ = nullptr;  // HANDLE stored as void* to avoid windows.h in header
#else
    int socket_fd_ = -1;
#endif
};

} // namespace exv::helper
