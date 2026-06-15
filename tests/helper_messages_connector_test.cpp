#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_connector.hpp"
#include "helper/common/helper_client.hpp"

#include <nlohmann/json.hpp>
#include <cassert>
#include <iostream>
#include <string>
#include <utility>
#include <vector>

using namespace exv::helper;
using json = nlohmann::json;

namespace ecnuvpn {
namespace logger {

void init() {}
void write(const std::string &, const std::string &) {}
void info(const std::string &) {}
void error(const std::string &) {}
void warn(const std::string &) {}
void event(const std::string &, const std::string &, const std::string &,
           const std::string &,
           const std::vector<std::pair<std::string, std::string>> &) {}
void show_logs(int) {}
std::vector<std::string> tail(int) { return {}; }

} // namespace logger
} // namespace ecnuvpn

// ---- Serialization round-trip tests ----

static void test_hello_request_roundtrip() {
    HelloRequest req;
    json j = req;
    assert(j.is_object());
    assert(!j.contains("client_version"));
    auto parsed = hello_request_from_json(j);
    (void)parsed;
    std::cout << "  PASS hello_request_roundtrip\n";
}

static void test_hello_response_roundtrip() {
    HelloResponse resp;
    resp.capabilities = {"tunnel_device_create", "route_apply"};
    resp.mode = HelperMode::Resident;
    resp.startup_context.launch_mode = "service";
    resp.session_state.active = false;
    resp.core_lease.active = true;
    resp.core_lease.lease_id = "lease-123";
    resp.core_lease.core_pid = 4321;
    resp.core_lease.purpose = "connect";
    resp.core_lease.last_seen_state = "authenticating";
    resp.task_queue.idle = false;
    resp.task_queue.current_job_id = "job-123";
    resp.task_queue.pending_jobs = 3;
    json j = resp;
    assert(!j.contains("server_version"));
    assert(j["capabilities"].size() == 2);
    assert(j["mode"] == static_cast<uint32_t>(HelperMode::Resident));
    assert(j["startup_context"]["launch_mode"] == "service");
    assert(j["core_lease"]["lease_id"] == "lease-123");
    assert(j["core_lease"]["core_pid"] == 4321);
    assert(j["task_queue"]["idle"] == false);
    assert(j["task_queue"]["pending_jobs"] == 3);
    auto parsed = hello_response_from_json(j);
    assert(parsed.capabilities.size() == 2);
    assert(parsed.mode == HelperMode::Resident);
    assert(parsed.startup_context.launch_mode == "service");
    assert(parsed.core_lease.active == true);
    assert(parsed.core_lease.lease_id == "lease-123");
    assert(parsed.core_lease.core_pid == 4321);
    assert(parsed.core_lease.purpose == "connect");
    assert(parsed.core_lease.last_seen_state == "authenticating");
    assert(parsed.task_queue.idle == false);
    assert(parsed.task_queue.current_job_id == "job-123");
    assert(parsed.task_queue.pending_jobs == 3);
    std::cout << "  PASS hello_response_roundtrip\n";
}

static void test_start_session_roundtrip() {
    StartSessionRequest req;
    req.profile_id.value = "profile-1";
    req.mode = HelperMode::Transient;
    json j = req;
    assert(j["profile_id"] == "profile-1");
    assert(j["mode"] == 1);
    auto parsed = start_session_request_from_json(j);
    assert(parsed.profile_id.value == "profile-1");
    assert(parsed.mode == HelperMode::Transient);
    std::cout << "  PASS start_session_roundtrip\n";
}

static void test_start_session_response_roundtrip() {
    StartSessionResponse resp;
    resp.session_id.value = "sess-abc";
    json j = resp;
    assert(j["session_id"] == "sess-abc");
    auto parsed = start_session_response_from_json(j);
    assert(parsed.session_id.value == "sess-abc");
    std::cout << "  PASS start_session_response_roundtrip\n";
}

static void test_prepare_tunnel_device_roundtrip() {
    PrepareTunnelDeviceRequest req;
    req.session_id.value = "s1";
    req.adapter_name = "Wintun";
    json j = req;
    assert(j["adapter_name"] == "Wintun");
    auto parsed = prepare_tunnel_device_request_from_json(j);
    assert(parsed.session_id.value == "s1");
    assert(parsed.adapter_name == "Wintun");

    PrepareTunnelDeviceResponse resp;
    resp.device_path = "//./Wintun/0";
    resp.mtu = 1500;
    json j2 = resp;
    assert(j2["device_path"] == "//./Wintun/0");
    assert(j2["mtu"] == 1500);
    auto parsed2 = prepare_tunnel_device_response_from_json(j2);
    assert(parsed2.device_path == "//./Wintun/0");
    assert(parsed2.mtu == 1500);
    std::cout << "  PASS prepare_tunnel_device_roundtrip\n";
}

static void test_apply_tunnel_config_roundtrip() {
    ApplyTunnelConfigRequest req;
    req.config.session_id.value = "s2";
    req.config.interface_address = "10.0.0.2/24";
    req.config.routes.push_back({"0.0.0.0/0", "10.0.0.1", 100});
    req.config.server_bypass_ips = {"192.0.2.10", "192.0.2.11/32"};
    req.config.dns.servers = {"8.8.8.8"};
    req.config.dns.search_domain = "vpn.local";
    req.config.enable_kill_switch = true;
    json j = req;
    assert(j["config"]["interface_address"] == "10.0.0.2/24");
    assert(j["config"]["routes"].size() == 1);
    assert(j["config"]["server_bypass_ips"].size() == 2);
    assert(j["config"]["server_bypass_ips"][0] == "192.0.2.10");
    assert(j["config"]["server_bypass_ips"][1] == "192.0.2.11/32");
    assert(j["config"]["dns"]["servers"][0] == "8.8.8.8");
    assert(j["config"]["enable_kill_switch"] == true);
    auto parsed = apply_tunnel_config_request_from_json(j);
    assert(parsed.config.session_id.value == "s2");
    assert(parsed.config.routes.size() == 1);
    assert(parsed.config.routes[0].destination == "0.0.0.0/0");
    assert(parsed.config.server_bypass_ips.size() == 2);
    assert(parsed.config.server_bypass_ips[0] == "192.0.2.10");
    assert(parsed.config.server_bypass_ips[1] == "192.0.2.11/32");
    assert(parsed.config.dns.servers.size() == 1);
    assert(parsed.config.enable_kill_switch == true);
    std::cout << "  PASS apply_tunnel_config_roundtrip\n";
}

static void test_heartbeat_roundtrip() {
    HeartbeatRequest req;
    req.session_id.value = "hb-sess";
    req.core_phase = "Connected";
    json j = req;
    assert(j["core_phase"] == "Connected");
    auto parsed = heartbeat_request_from_json(j);
    assert(parsed.session_id.value == "hb-sess");
    assert(parsed.core_phase == "Connected");

    HeartbeatResponse resp;
    resp.ok = true;
    resp.warning = "low-mtu";
    json j2 = resp;
    assert(j2["ok"] == true);
    assert(j2["warning"] == "low-mtu");
    auto parsed2 = heartbeat_response_from_json(j2);
    assert(parsed2.ok == true);
    assert(parsed2.warning.has_value());
    assert(parsed2.warning.value() == "low-mtu");
    std::cout << "  PASS heartbeat_roundtrip\n";
}

static void test_cleanup_roundtrip() {
    CleanupRequest req;
    req.session_id.value = "cl-sess";
    req.policy.remove_routes = true;
    req.policy.remove_dns = false;
    req.policy.remove_adapter = true;
    req.policy.remove_firewall_rules = false;
    json j = req;
    assert(j["policy"]["remove_adapter"] == true);
    assert(j["policy"]["remove_dns"] == false);
    auto parsed = cleanup_request_from_json(j);
    assert(parsed.session_id.value == "cl-sess");
    assert(parsed.policy.remove_routes == true);
    assert(parsed.policy.remove_dns == false);
    assert(parsed.policy.remove_adapter == true);
    assert(parsed.policy.remove_firewall_rules == false);

    CleanupResponse resp;
    resp.success = false;
    resp.errors = {"route-failed", "dns-timeout"};
    json j2 = resp;
    assert(j2["errors"].size() == 2);
    auto parsed2 = cleanup_response_from_json(j2);
    assert(parsed2.success == false);
    assert(parsed2.errors.size() == 2);
    std::cout << "  PASS cleanup_roundtrip\n";
}

static void test_snapshot_roundtrip() {
    GetSnapshotRequest req;
    json j = req;
    (void)j;  // empty object
    auto parsed = get_snapshot_request_from_json(j);

    SessionSnapshot snap;
    snap.session_id.value = "snap-sess";
    snap.core_phase = "Connected";
    snap.last_heartbeat = std::chrono::steady_clock::now();
    snap.managed_resources = {"route:0.0.0.0/0", "adapter:Wintun"};
    json j2 = snap;
    assert(j2["core_phase"] == "Connected");
    assert(j2["managed_resources"].size() == 2);

    GetSnapshotResponse resp;
    resp.sessions.push_back(snap);
    json j3 = resp;
    assert(j3["sessions"].size() == 1);
    auto parsed3 = get_snapshot_response_from_json(j3);
    assert(parsed3.sessions.size() == 1);
    assert(parsed3.sessions[0].session_id.value == "snap-sess");
    std::cout << "  PASS snapshot_roundtrip\n";
}

static void test_shutdown_roundtrip() {
    ShutdownRequest req;
    req.session_id.value = "shutdown-sess";
    req.policy.remove_adapter = true;
    json j = req;
    assert(j["session_id"] == "shutdown-sess");
    assert(j["policy"]["remove_adapter"] == true);
    auto parsed = shutdown_request_from_json(j);
    assert(parsed.session_id.value == "shutdown-sess");
    assert(parsed.policy.remove_adapter == true);

    ShutdownResponse resp;
    resp.cleanup_success = true;
    resp.exiting = true;
    json j2 = resp;
    assert(j2["cleanup_success"] == true);
    assert(j2["exiting"] == true);
    auto parsed2 = shutdown_response_from_json(j2);
    assert(parsed2.cleanup_success == true);
    assert(parsed2.exiting == true);
    std::cout << "  PASS shutdown_roundtrip\n";
}

static void test_core_lease_state_roundtrip() {
    CoreLeaseState state;
    state.active = true;
    state.lease_id = "lease-abc";
    state.core_pid = 2468;
    state.purpose = "connect";
    state.last_seen_state = "connected";
    json j = state;
    assert(j["active"] == true);
    assert(j["lease_id"] == "lease-abc");
    assert(j["core_pid"] == 2468);
    assert(j["purpose"] == "connect");
    assert(j["last_seen_state"] == "connected");
    auto parsed = core_lease_state_from_json(j);
    assert(parsed.active == true);
    assert(parsed.lease_id == "lease-abc");
    assert(parsed.core_pid == 2468);
    assert(parsed.purpose == "connect");
    assert(parsed.last_seen_state == "connected");
    std::cout << "  PASS core_lease_state_roundtrip\n";
}

static void test_task_queue_state_roundtrip() {
    TaskQueueState state;
    state.idle = false;
    state.current_job_id = "job-abc";
    state.pending_jobs = 7;
    json j = state;
    assert(j["idle"] == false);
    assert(j["current_job_id"] == "job-abc");
    assert(j["pending_jobs"] == 7);
    auto parsed = task_queue_state_from_json(j);
    assert(parsed.idle == false);
    assert(parsed.current_job_id == "job-abc");
    assert(parsed.pending_jobs == 7);
    std::cout << "  PASS task_queue_state_roundtrip\n";
}

static void test_inspect_roundtrip() {
    InspectRequest req;
    json j = req;
    assert(j.is_object());
    auto parsed_req = inspect_request_from_json(j);
    (void)parsed_req;

    InspectResponse resp;
    resp.capabilities = {"tunnel_device_create"};
    resp.mode = HelperMode::Transient;
    resp.startup_context.launch_mode = "oneshot";
    resp.session_state.active = false;
    resp.core_lease.active = true;
    resp.core_lease.lease_id = "lease-inspect";
    resp.task_queue.idle = true;
    json j2 = resp;
    assert(j2["capabilities"].size() == 1);
    assert(j2["mode"] == static_cast<uint32_t>(HelperMode::Transient));
    assert(j2["startup_context"]["launch_mode"] == "oneshot");
    assert(j2["core_lease"]["lease_id"] == "lease-inspect");
    auto parsed_resp = inspect_response_from_json(j2);
    assert(parsed_resp.capabilities.size() == 1);
    assert(parsed_resp.mode == HelperMode::Transient);
    assert(parsed_resp.startup_context.launch_mode == "oneshot");
    assert(parsed_resp.core_lease.active == true);
    assert(parsed_resp.core_lease.lease_id == "lease-inspect");
    assert(parsed_resp.task_queue.idle == true);
    std::cout << "  PASS inspect_roundtrip\n";
}

static void test_acquire_core_lease_roundtrip() {
    AcquireCoreLeaseRequest req;
    req.core_pid = 1357;
    req.purpose = "connect";
    json j = req;
    assert(j["core_pid"] == 1357);
    assert(j["purpose"] == "connect");
    auto parsed_req = acquire_core_lease_request_from_json(j);
    assert(parsed_req.core_pid == 1357);
    assert(parsed_req.purpose == "connect");

    AcquireCoreLeaseResponse resp;
    resp.accepted = true;
    resp.lease_id = "lease-xyz";
    resp.mode = "oneshot";
    json j2 = resp;
    assert(j2["accepted"] == true);
    assert(j2["lease_id"] == "lease-xyz");
    assert(j2["mode"] == "oneshot");
    auto parsed_resp = acquire_core_lease_response_from_json(j2);
    assert(parsed_resp.accepted == true);
    assert(parsed_resp.lease_id == "lease-xyz");
    assert(parsed_resp.mode == "oneshot");
    std::cout << "  PASS acquire_core_lease_roundtrip\n";
}

static void test_keep_alive_roundtrip() {
    KeepAliveRequest req;
    req.lease_id = "lease-xyz";
    req.state = "connected";
    json j = req;
    assert(j["lease_id"] == "lease-xyz");
    assert(j["state"] == "connected");
    auto parsed_req = keep_alive_request_from_json(j);
    assert(parsed_req.lease_id == "lease-xyz");
    assert(parsed_req.state == "connected");

    KeepAliveResponse resp;
    resp.ok = true;
    resp.warning = "late";
    json j2 = resp;
    assert(j2["ok"] == true);
    assert(j2["warning"] == "late");
    auto parsed_resp = keep_alive_response_from_json(j2);
    assert(parsed_resp.ok == true);
    assert(parsed_resp.warning.has_value());
    assert(parsed_resp.warning.value() == "late");
    std::cout << "  PASS keep_alive_roundtrip\n";
}

static void test_release_core_lease_roundtrip() {
    ReleaseCoreLeaseRequest req;
    req.lease_id = "lease-xyz";
    req.exit_if_oneshot = false;
    json j = req;
    assert(j["lease_id"] == "lease-xyz");
    assert(j["exit_if_oneshot"] == false);
    auto parsed_req = release_core_lease_request_from_json(j);
    assert(parsed_req.lease_id == "lease-xyz");
    assert(parsed_req.exit_if_oneshot == false);

    ReleaseCoreLeaseResponse resp;
    resp.released = true;
    resp.exiting = false;
    json j2 = resp;
    assert(j2["released"] == true);
    assert(j2["exiting"] == false);
    auto parsed_resp = release_core_lease_response_from_json(j2);
    assert(parsed_resp.released == true);
    assert(parsed_resp.exiting == false);
    std::cout << "  PASS release_core_lease_roundtrip\n";
}

static void test_service_maintenance_roundtrip() {
    InstallServiceRequest install_req;
    json install_json = install_req;
    assert(install_json.is_object());
    auto parsed_install_req = install_service_request_from_json(install_json);
    (void)parsed_install_req;

    InstallServiceResponse install_resp;
    install_resp.success = true;
    install_resp.exit_code = 0;
    install_resp.message = "installed";
    json install_resp_json = install_resp;
    assert(install_resp_json["success"] == true);
    assert(install_resp_json["exit_code"] == 0);
    assert(install_resp_json["message"] == "installed");
    auto parsed_install_resp =
        install_service_response_from_json(install_resp_json);
    assert(parsed_install_resp.success == true);
    assert(parsed_install_resp.exit_code == 0);
    assert(parsed_install_resp.message == "installed");

    UninstallServiceRequest uninstall_req;
    json uninstall_json = uninstall_req;
    assert(uninstall_json.is_object());
    auto parsed_uninstall_req =
        uninstall_service_request_from_json(uninstall_json);
    (void)parsed_uninstall_req;

    UninstallServiceResponse uninstall_resp;
    uninstall_resp.success = false;
    uninstall_resp.exit_code = 2;
    uninstall_resp.message = "active vpn session";
    json uninstall_resp_json = uninstall_resp;
    assert(uninstall_resp_json["success"] == false);
    assert(uninstall_resp_json["exit_code"] == 2);
    assert(uninstall_resp_json["message"] == "active vpn session");
    auto parsed_uninstall_resp =
        uninstall_service_response_from_json(uninstall_resp_json);
    assert(parsed_uninstall_resp.success == false);
    assert(parsed_uninstall_resp.exit_code == 2);
    assert(parsed_uninstall_resp.message == "active vpn session");

    std::cout << "  PASS service_maintenance_roundtrip\n";
}

static void test_cleanup_lease_handoff_roundtrip() {
    CleanupLease lease;
    lease.cleanup_lease_id = "cleanup-lease-1";
    CleanupLeaseSession session;
    session.session_id.value = "ses-1";
    session.profile_id.value = "profile-1";
    session.mode = HelperMode::Transient;
    session.core_phase = "Connected";
    session.cleanup_policy.remove_adapter = true;
    session.managed_resources.push_back({"adapter", "ECNU-VPN"});
    session.managed_resources.push_back({"route", "10.0.0.0/8"});
    lease.sessions.push_back(session);

    json lease_json = lease;
    assert(lease_json["cleanup_lease_id"] == "cleanup-lease-1");
    assert(lease_json["sessions"].size() == 1);
    assert(lease_json["sessions"][0]["managed_resources"].size() == 2);
    auto parsed_lease = cleanup_lease_from_json(lease_json);
    assert(parsed_lease.cleanup_lease_id == "cleanup-lease-1");
    assert(parsed_lease.sessions.size() == 1);
    assert(parsed_lease.sessions[0].session_id.value == "ses-1");
    assert(parsed_lease.sessions[0].managed_resources[0].type == "adapter");

    ExportCleanupLeaseRequest export_req;
    json export_req_json = export_req;
    assert(export_req_json.is_object());
    auto parsed_export_req =
        export_cleanup_lease_request_from_json(export_req_json);
    (void)parsed_export_req;

    ExportCleanupLeaseResponse export_resp;
    export_resp.lease = lease;
    export_resp.has_active_session = true;
    json export_resp_json = export_resp;
    assert(export_resp_json["has_active_session"] == true);
    auto parsed_export_resp =
        export_cleanup_lease_response_from_json(export_resp_json);
    assert(parsed_export_resp.has_active_session == true);
    assert(parsed_export_resp.lease.sessions.size() == 1);

    HandoffSessionRequest handoff_req;
    handoff_req.lease = lease;
    json handoff_req_json = handoff_req;
    assert(handoff_req_json["lease"]["cleanup_lease_id"] == "cleanup-lease-1");
    auto parsed_handoff_req =
        handoff_session_request_from_json(handoff_req_json);
    assert(parsed_handoff_req.lease.sessions.size() == 1);

    HandoffSessionResponse handoff_resp;
    handoff_resp.adopted = true;
    handoff_resp.session_ids.push_back(session.session_id);
    handoff_resp.message = "adopted";
    json handoff_resp_json = handoff_resp;
    assert(handoff_resp_json["adopted"] == true);
    auto parsed_handoff_resp =
        handoff_session_response_from_json(handoff_resp_json);
    assert(parsed_handoff_resp.adopted == true);
    assert(parsed_handoff_resp.session_ids[0].value == "ses-1");

    FinalizeHandoffRequest finalize_req;
    finalize_req.exit = true;
    json finalize_req_json = finalize_req;
    assert(finalize_req_json["exit"] == true);
    auto parsed_finalize_req =
        finalize_handoff_request_from_json(finalize_req_json);
    assert(parsed_finalize_req.exit == true);

    FinalizeHandoffResponse finalize_resp;
    finalize_resp.finalized = true;
    finalize_resp.exiting = true;
    json finalize_resp_json = finalize_resp;
    auto parsed_finalize_resp =
        finalize_handoff_response_from_json(finalize_resp_json);
    assert(parsed_finalize_resp.finalized == true);
    assert(parsed_finalize_resp.exiting == true);

    std::cout << "  PASS cleanup_lease_handoff_roundtrip\n";
}

static void test_helper_request_response_roundtrip() {
    HelperRequest req;
    req.op = HelperOp::StartSession;
    req.payload_json = "{\"profile_id\":\"p1\"}";
    json j = req;
    assert(j["op"] == static_cast<uint32_t>(HelperOp::StartSession));
    assert(j["payload_json"] == "{\"profile_id\":\"p1\"}");
    auto parsed = helper_request_from_json(j);
    assert(parsed.op == HelperOp::StartSession);
    assert(parsed.payload_json == "{\"profile_id\":\"p1\"}");

    HelperResponse resp;
    resp.op = HelperOp::StartSession;
    resp.success = true;
    resp.error_code = "";
    resp.error_message = "";
    resp.payload_json = "{\"session_id\":\"s1\"}";
    json j2 = resp;
    assert(j2["success"] == true);
    auto parsed2 = helper_response_from_json(j2);
    assert(parsed2.op == HelperOp::StartSession);
    assert(parsed2.success == true);
    assert(parsed2.payload_json == "{\"session_id\":\"s1\"}");
    std::cout << "  PASS helper_request_response_roundtrip\n";
}

static void test_connector_factory_create() {
    auto connector = HelperConnector::create();
    assert(connector != nullptr);
    std::cout << "  PASS connector_factory_create\n";
}

int main() {
    std::cout << "=== helper_messages serialization tests ===\n";
    test_hello_request_roundtrip();
    test_hello_response_roundtrip();
    test_start_session_roundtrip();
    test_start_session_response_roundtrip();
    test_prepare_tunnel_device_roundtrip();
    test_apply_tunnel_config_roundtrip();
    test_heartbeat_roundtrip();
    test_cleanup_roundtrip();
    test_snapshot_roundtrip();
    test_shutdown_roundtrip();
    test_core_lease_state_roundtrip();
    test_task_queue_state_roundtrip();
    test_inspect_roundtrip();
    test_acquire_core_lease_roundtrip();
    test_keep_alive_roundtrip();
    test_release_core_lease_roundtrip();
    test_service_maintenance_roundtrip();
    test_cleanup_lease_handoff_roundtrip();
    test_helper_request_response_roundtrip();

    std::cout << "\n=== HelperConnector factory tests ===\n";
    test_connector_factory_create();

    std::cout << "\nAll tests passed.\n";
    return 0;
}
