#pragma once
#include <memory>
#include <functional>
#include "helper_messages.hpp"
#include "helper_error.hpp"

namespace exv::helper {

class HelperClient {
public:
    virtual ~HelperClient() = default;

    // Connection management
    virtual bool connect() = 0;
    virtual void disconnect() = 0;
    virtual bool is_connected() const = 0;

    // V2 protocol
    virtual HelloResponse hello(const HelloRequest& req) = 0;
    virtual StartSessionResponse start_session(const StartSessionRequest& req) = 0;
    virtual PrepareTunnelDeviceResponse prepare_tunnel_device(const PrepareTunnelDeviceRequest& req) = 0;
    virtual ApplyTunnelConfigResponse apply_tunnel_config(const ApplyTunnelConfigRequest& req) = 0;
    virtual HeartbeatResponse heartbeat(const HeartbeatRequest& req) = 0;
    virtual CleanupResponse cleanup(const CleanupRequest& req) = 0;
    virtual GetSnapshotResponse get_snapshot(const GetSnapshotRequest& req) = 0;
    virtual EndSessionResponse end_session(const EndSessionRequest& req) = 0;

    // Callback for helper disconnection
    using DisconnectCallback = std::function<void()>;
    virtual void set_disconnect_callback(DisconnectCallback cb) = 0;
};

} // namespace exv::helper
