#include "fake_helper.hpp"

namespace exv::test {

FakeHelper::FakeHelper() = default;
FakeHelper::~FakeHelper() = default;

bool FakeHelper::connect() {
    connect_count_++;
    connected_ = !fail_next_;
    fail_next_ = false;
    if (connected_) ipc_lost_ = false;
    return connected_;
}

void FakeHelper::disconnect() {
    connected_ = false;
}

bool FakeHelper::is_connected() const {
    return connected_;
}

helper::HelloResponse FakeHelper::hello(const helper::HelloRequest& req) {
    (void)req;
    helper::HelloResponse resp;
    resp.capabilities = {"tunnel_device_create", "route_apply", "dns_apply", "route_cleanup"};
    resp.mode = helper::HelperMode::Transient;
    return resp;
}

helper::StartSessionResponse FakeHelper::start_session(const helper::StartSessionRequest& req) {
    helper::StartSessionResponse resp;
    if (start_session_fail_) {
        // Return empty session_id to signal failure
        resp.session_id.value = "";
        return resp;
    }
    resp.session_id.value = "fake-session-" + std::to_string(sessions_.size() + 1);
    helper::SessionLease lease;
    lease.session_id = resp.session_id;
    lease.profile_id = req.profile_id;
    lease.mode = req.mode;
    lease.last_heartbeat = std::chrono::steady_clock::now();
    sessions_[resp.session_id] = lease;
    return resp;
}

helper::PrepareTunnelDeviceResponse FakeHelper::prepare_tunnel_device(const helper::PrepareTunnelDeviceRequest& req) {
    helper::PrepareTunnelDeviceResponse resp;
    resp.device_path = "//./FakeTun/" + req.session_id.value;
    resp.mtu = 1400;
    return resp;
}

helper::ApplyTunnelConfigResponse FakeHelper::apply_tunnel_config(const helper::ApplyTunnelConfigRequest& req) {
    helper::ApplyTunnelConfigResponse resp;
    resp.success = !fail_next_ && !apply_config_fail_;
    if (!resp.success) {
        resp.error_message = apply_config_fail_ ? "Simulated apply_config failure" : "Simulated fail_next";
    }
    fail_next_ = false;
    return resp;
}

helper::HeartbeatResponse FakeHelper::heartbeat(const helper::HeartbeatRequest& req) {
    heartbeat_count_++;
    helper::HeartbeatResponse resp;
    if (heartbeat_fail_after_ >= 0 && heartbeat_count_ > heartbeat_fail_after_) {
        resp.ok = false;
        resp.warning = "Simulated heartbeat failure";
    }
    return resp;
}

helper::CleanupResponse FakeHelper::cleanup(const helper::CleanupRequest& req) {
    cleanup_requests_.push_back(req);
    helper::CleanupResponse resp;
    resp.success = true;
    sessions_.erase(req.session_id);
    return resp;
}

helper::GetSnapshotResponse FakeHelper::get_snapshot(const helper::GetSnapshotRequest& req) {
    helper::GetSnapshotResponse resp;
    return resp;
}

helper::ShutdownResponse FakeHelper::shutdown(const helper::ShutdownRequest& req) {
    helper::ShutdownResponse resp;
    resp.cleanup_success = true;
    sessions_.erase(req.session_id);
    return resp;
}

void FakeHelper::set_disconnect_callback(DisconnectCallback cb) {
    disconnect_cb_ = std::move(cb);
}

void FakeHelper::set_should_fail_next(bool fail) {
    fail_next_ = fail;
}

void FakeHelper::simulate_disconnect() {
    connected_ = false;
    if (disconnect_cb_) disconnect_cb_();
}

void FakeHelper::simulate_ipc_lost() {
    ipc_lost_ = true;
    connected_ = false;
    if (disconnect_cb_) disconnect_cb_();
}

void FakeHelper::set_heartbeat_fail_after(int count) {
    heartbeat_fail_after_ = count;
}

void FakeHelper::set_start_session_fail(bool fail) {
    start_session_fail_ = fail;
}

void FakeHelper::set_apply_config_fail(bool fail) {
    apply_config_fail_ = fail;
}

int FakeHelper::connect_count() const { return connect_count_; }
int FakeHelper::heartbeat_count() const { return heartbeat_count_; }
bool FakeHelper::ipc_lost() const { return ipc_lost_; }

std::vector<helper::SessionId> FakeHelper::active_sessions() const {
    std::vector<helper::SessionId> result;
    for (auto& [id, lease] : sessions_) result.push_back(id);
    return result;
}

std::vector<helper::CleanupRequest> FakeHelper::cleanup_requests() const {
    return cleanup_requests_;
}

} // namespace exv::test
