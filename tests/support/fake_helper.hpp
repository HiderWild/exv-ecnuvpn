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
    helper::InspectResponse inspect(const helper::InspectRequest& req) override;
    helper::AcquireCoreLeaseResponse acquire_core_lease(const helper::AcquireCoreLeaseRequest& req) override;
    helper::KeepAliveResponse keep_alive(const helper::KeepAliveRequest& req) override;
    helper::ReleaseCoreLeaseResponse release_core_lease(const helper::ReleaseCoreLeaseRequest& req) override;

    void set_disconnect_callback(DisconnectCallback cb) override;

    // Test control
    void set_should_fail_next(bool fail);
    void simulate_disconnect();
    void simulate_ipc_lost();
    void set_heartbeat_fail_after(int count);
    void set_start_session_fail(bool fail);
    void set_prepare_device_fail(std::string error_code, std::string error_message);
    void set_apply_config_fail(bool fail);
    void set_apply_config_fail(std::string error_code, std::string error_message);
    void set_require_prepare_before_apply(bool require);

    // Inspection
    int connect_count() const;
    int hello_count() const;
    int prepare_count() const;
    int apply_count() const;
    int shutdown_count() const;
    int heartbeat_count() const;
    int acquire_core_lease_count() const;
    int keep_alive_count() const;
    int release_core_lease_count() const;
    bool ipc_lost() const;
    std::vector<helper::SessionId> active_sessions() const;
    std::vector<helper::CleanupRequest> cleanup_requests() const;
    std::vector<helper::ShutdownRequest> shutdown_requests() const;
    std::vector<helper::ApplyTunnelConfigRequest> apply_requests() const;

private:
    bool connected_ = false;
    bool fail_next_ = false;
    bool start_session_fail_ = false;
    bool prepare_device_fail_ = false;
    std::string prepare_device_error_code_;
    std::string prepare_device_error_message_;
    bool apply_config_fail_ = false;
    std::string apply_config_error_code_;
    std::string apply_config_error_message_;
    std::string apply_config_error_target_;
    std::uint32_t apply_config_system_error_ = 0;
    bool require_prepare_before_apply_ = false;
    bool ipc_lost_ = false;
    int prepare_count_ = 0;
    int apply_count_ = 0;
    int shutdown_count_ = 0;
    int heartbeat_count_ = 0;
    int hello_count_ = 0;
    int acquire_core_lease_count_ = 0;
    int keep_alive_count_ = 0;
    int release_core_lease_count_ = 0;
    int heartbeat_fail_after_ = -1;
    int connect_count_ = 0;
    bool core_lease_active_ = false;
    int core_lease_pid_ = 0;
    std::string core_lease_purpose_;
    std::string core_lease_last_seen_state_;
    std::string core_lease_id_;
    DisconnectCallback disconnect_cb_;
    std::map<helper::SessionId, helper::SessionLease> sessions_;
    std::map<helper::SessionId, bool> prepared_sessions_;
    std::vector<helper::CleanupRequest> cleanup_requests_;
    std::vector<helper::ShutdownRequest> shutdown_requests_;
    std::vector<helper::ApplyTunnelConfigRequest> apply_requests_;
};

} // namespace exv::test
