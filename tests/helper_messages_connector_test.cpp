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
    json j = resp;
    assert(!j.contains("server_version"));
    assert(j["capabilities"].size() == 2);
    assert(j["mode"] == static_cast<uint32_t>(HelperMode::Resident));
    assert(j["startup_context"]["launch_mode"] == "service");
    auto parsed = hello_response_from_json(j);
    assert(parsed.capabilities.size() == 2);
    assert(parsed.mode == HelperMode::Resident);
    assert(parsed.startup_context.launch_mode == "service");
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
    test_helper_request_response_roundtrip();

    std::cout << "\n=== HelperConnector factory tests ===\n";
    test_connector_factory_create();

    std::cout << "\nAll tests passed.\n";
    return 0;
}
