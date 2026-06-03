#include "helper_common/helper_messages.hpp"
#include "helper_common/helper_connector.hpp"
#include "helper_common/helper_client.hpp"

#include <nlohmann/json.hpp>
#include <cassert>
#include <iostream>
#include <string>

using namespace exv::helper;
using json = nlohmann::json;

// ---- Serialization round-trip tests ----

static void test_hello_request_roundtrip() {
    HelloRequest req;
    req.client_version = 42;
    json j = req;
    assert(j["client_version"] == 42);
    auto parsed = hello_request_from_json(j);
    assert(parsed.client_version == 42);
    std::cout << "  PASS hello_request_roundtrip\n";
}

static void test_hello_response_roundtrip() {
    HelloResponse resp;
    resp.server_version = 2;
    resp.capabilities = {"tunnel_device_create", "route_apply"};
    resp.mode = HelperMode::Resident;
    json j = resp;
    assert(j["server_version"] == 2);
    assert(j["capabilities"].size() == 2);
    assert(j["mode"] == static_cast<uint32_t>(HelperMode::Resident));
    auto parsed = hello_response_from_json(j);
    assert(parsed.server_version == 2);
    assert(parsed.capabilities.size() == 2);
    assert(parsed.mode == HelperMode::Resident);
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
    req.config.dns.servers = {"8.8.8.8"};
    req.config.dns.search_domain = "vpn.local";
    req.config.enable_kill_switch = true;
    json j = req;
    assert(j["config"]["interface_address"] == "10.0.0.2/24");
    assert(j["config"]["routes"].size() == 1);
    assert(j["config"]["dns"]["servers"][0] == "8.8.8.8");
    assert(j["config"]["enable_kill_switch"] == true);
    auto parsed = apply_tunnel_config_request_from_json(j);
    assert(parsed.config.session_id.value == "s2");
    assert(parsed.config.routes.size() == 1);
    assert(parsed.config.routes[0].destination == "0.0.0.0/0");
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

static void test_end_session_roundtrip() {
    EndSessionRequest req;
    req.session_id.value = "end-sess";
    json j = req;
    assert(j["session_id"] == "end-sess");
    auto parsed = end_session_request_from_json(j);
    assert(parsed.session_id.value == "end-sess");

    EndSessionResponse resp;
    resp.success = true;
    json j2 = resp;
    assert(j2["success"] == true);
    auto parsed2 = end_session_response_from_json(j2);
    assert(parsed2.success == true);
    std::cout << "  PASS end_session_roundtrip\n";
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

// ---- HelperConnector / StubHelperClient tests ----

static void test_stub_connector_create() {
    auto connector = HelperConnector::create_stub();
    assert(connector != nullptr);
    assert(connector->is_helper_available());
    std::cout << "  PASS stub_connector_create\n";
}

static void test_stub_connector_connect() {
    auto connector = HelperConnector::create_stub();
    HelperConnectorConfig config;
    auto client = connector->connect(config);
    assert(client != nullptr);
    assert(client->is_connected());
    std::cout << "  PASS stub_connector_connect\n";
}

static void test_stub_client_hello() {
    auto connector = HelperConnector::create_stub();
    HelperConnectorConfig config;
    auto client = connector->connect(config);

    HelloRequest req;
    req.client_version = PROTOCOL_VERSION;
    auto resp = client->hello(req);
    assert(resp.server_version == PROTOCOL_VERSION);
    assert(resp.capabilities.size() == 4);
    assert(resp.mode == HelperMode::Transient);
    std::cout << "  PASS stub_client_hello\n";
}

static void test_stub_client_session_lifecycle() {
    auto connector = HelperConnector::create_stub();
    HelperConnectorConfig config;
    auto client = connector->connect(config);

    // Start session
    StartSessionRequest start_req;
    start_req.profile_id.value = "test-profile";
    auto start_resp = client->start_session(start_req);
    assert(!start_resp.session_id.value.empty());

    // Prepare tunnel device
    PrepareTunnelDeviceRequest prep_req;
    prep_req.session_id = start_resp.session_id;
    prep_req.adapter_name = "Wintun";
    auto prep_resp = client->prepare_tunnel_device(prep_req);
    assert(!prep_resp.device_path.empty());
    assert(prep_resp.mtu == 1400);

    // Apply tunnel config
    ApplyTunnelConfigRequest cfg_req;
    cfg_req.config.session_id = start_resp.session_id;
    cfg_req.config.interface_address = "10.0.0.2/24";
    auto cfg_resp = client->apply_tunnel_config(cfg_req);
    assert(cfg_resp.success);

    // Heartbeat
    HeartbeatRequest hb_req;
    hb_req.session_id = start_resp.session_id;
    hb_req.core_phase = "Connected";
    auto hb_resp = client->heartbeat(hb_req);
    assert(hb_resp.ok);

    // Get snapshot
    GetSnapshotRequest snap_req;
    auto snap_resp = client->get_snapshot(snap_req);
    (void)snap_resp;

    // End session
    EndSessionRequest end_req;
    end_req.session_id = start_resp.session_id;
    auto end_resp = client->end_session(end_req);
    assert(end_resp.success);

    // Disconnect
    client->disconnect();
    assert(!client->is_connected());

    std::cout << "  PASS stub_client_session_lifecycle\n";
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
    test_end_session_roundtrip();
    test_helper_request_response_roundtrip();

    std::cout << "\n=== HelperConnector / StubHelperClient tests ===\n";
    test_stub_connector_create();
    test_stub_connector_connect();
    test_stub_client_hello();
    test_stub_client_session_lifecycle();

    std::cout << "\nAll tests passed.\n";
    return 0;
}
