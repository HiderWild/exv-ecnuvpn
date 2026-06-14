#pragma once
#include "helper/common/helper_client.hpp"
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_session_lease.hpp"
#include <queue>
#include <map>

namespace exv::test {

class FakeHelper : public exv::helper::HelperClient {
public:
    FakeHelper();
    ~FakeHelper() override;

    // HelperClient interface
    bool connect() override;
    void disconnect() override;
    bool is_connected() const override;

    helper::HelloResponse hello(const helper::HelloRequest& req) override;
    helper::StartSessionResponse start_session(const helper::StartSessionRequest& req) override;
    helper::PrepareTunnelDeviceResponse prepare_tunnel_device(const helper::PrepareTunnelDeviceRequest& req) override;
    helper::ApplyTunnelConfigResponse apply_tunnel_config(const helper::ApplyTunnelConfigRequest& req) override;
    helper::HeartbeatResponse heartbeat(const helper::HeartbeatRequest& req) override;
    helper::CleanupResponse cleanup(const helper::CleanupRequest& req) override;
    helper::GetSnapshotResponse get_snapshot(const helper::GetSnapshotRequest& req) override;
    helper::ShutdownResponse shutdown(const helper::ShutdownRequest& req) override;

    void set_disconnect_callback(DisconnectCallback cb) override;

    // Test control
    void set_should_fail_next(bool fail);
    void simulate_disconnect();
    void simulate_ipc_lost();
    void set_heartbeat_fail_after(int count);
    void set_start_session_fail(bool fail);
    void set_apply_config_fail(bool fail);

    // Inspection
    int connect_count() const;
    int heartbeat_count() const;
    bool ipc_lost() const;
    std::vector<helper::SessionId> active_sessions() const;
    std::vector<helper::CleanupRequest> cleanup_requests() const;

private:
    bool connected_ = false;
    bool fail_next_ = false;
    bool start_session_fail_ = false;
    bool apply_config_fail_ = false;
    bool ipc_lost_ = false;
    int heartbeat_count_ = 0;
    int heartbeat_fail_after_ = -1;
    int connect_count_ = 0;
    DisconnectCallback disconnect_cb_;
    std::map<helper::SessionId, helper::SessionLease> sessions_;
    std::vector<helper::CleanupRequest> cleanup_requests_;
};

} // namespace exv::test
