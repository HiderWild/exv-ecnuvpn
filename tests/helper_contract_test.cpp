// Helper single-protocol contract tests.

#include "contracts/generated/system_contract.hpp"
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_protocol.hpp"
#include "helper/helper_handler.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <string_view>
#include <vector>

using json = nlohmann::json;

namespace ecnuvpn::logger {
void info(const std::string &) {}
} // namespace ecnuvpn::logger

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

json read_json_file(const std::filesystem::path &path) {
  std::ifstream in(path);
  if (!in)
    throw std::runtime_error("failed to open " + path.string());
  json parsed;
  in >> parsed;
  return parsed;
}

template <typename Range>
bool contains(const Range &values, std::string_view value) {
  return std::find(values.begin(), values.end(), value) != values.end();
}

std::vector<std::string> helper_op_names(const json &helper) {
  std::vector<std::string> names;
  for (const auto &op : helper.at("ops")) {
    names.push_back(op.at("name").get<std::string>());
  }
  return names;
}

bool json_has_key_recursive(const json &node, std::string_view key) {
  if (node.is_object()) {
    for (auto it = node.begin(); it != node.end(); ++it) {
      if (it.key() == key)
        return true;
      if (json_has_key_recursive(it.value(), key))
        return true;
    }
  } else if (node.is_array()) {
    for (const auto &item : node) {
      if (json_has_key_recursive(item, key))
        return true;
    }
  }
  return false;
}

bool json_contains_credential_field(const json &node) {
  static constexpr std::string_view forbidden[] = {
      "password",      "passwd",       "cookie",      "token",
      "secret",        "credential",   "auth_key",    "auth_token",
      "authToken",     "session_cookie", "webvpn_cookie", "csrf_token",
      "bearer_token",  "api_key",      "apikey"};
  for (const auto field : forbidden) {
    if (json_has_key_recursive(node, field))
      return true;
  }
  return false;
}

exv::helper::HelperResponse dispatch_json(exv::helper::HelperHandler &handler,
                                          exv::helper::HelperOp op,
                                          const json &payload) {
  exv::helper::HelperRequest req;
  req.op = op;
  req.payload_json = payload.dump();
  return handler.handle(req);
}

int test_manifest_declares_single_helper_protocol() {
  bool ok = true;
  const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  const auto manifest =
      read_json_file(source_dir / "contracts" / "system.contract.json");
  const auto &helper = manifest.at("modules").at("helper");

  ok = expect(!helper.contains("protocol_version"),
              "helper manifest must not expose protocol_version") &&
       ok;
  ok = expect(!helper.contains("legacy_ops"),
              "helper manifest must not declare legacy_ops") &&
       ok;

  const std::vector<std::string> expected_ops = {
      "Hello",        "StartSession",       "PrepareTunnelDevice",
      "ApplyTunnelConfig", "Heartbeat",     "Cleanup",
      "GetSnapshot",  "Shutdown"};
  ok = expect(helper_op_names(helper) == expected_ops,
              "helper ops must be the single protocol op set") &&
       ok;

  ok = expect(contains(helper.at("messages"), "ShutdownRequest"),
              "manifest must list ShutdownRequest") &&
       ok;
  ok = expect(contains(helper.at("messages"), "ShutdownResponse"),
              "manifest must list ShutdownResponse") &&
       ok;
  ok = expect(!contains(helper.at("messages"), "EndSessionRequest"),
              "manifest must not list EndSessionRequest") &&
       ok;
  ok = expect(!contains(helper.at("messages"), "EndSessionResponse"),
              "manifest must not list EndSessionResponse") &&
       ok;

  return ok ? 0 : 1;
}

int test_generated_contract_matches_helper_manifest() {
  bool ok = true;
  using namespace exv::contracts::generated;

  ok = expect(is_helper_op("Hello"), "generated helper ops include Hello") && ok;
  ok = expect(is_helper_op("Shutdown"),
              "generated helper ops include Shutdown") &&
       ok;
  ok = expect(!is_helper_op("EndSession"),
              "generated helper ops exclude EndSession") &&
       ok;

  auto check = [&](std::string_view name, exv::helper::HelperOp op,
                   bool requires_session) {
    for (const auto &contract : HELPER_OP_CONTRACTS) {
      if (contract.name == name) {
        ok = expect(contract.code == static_cast<std::uint32_t>(op),
                    "generated op code must match wire enum") &&
             ok;
        ok = expect(contract.requires_session == requires_session,
                    "generated requires_session must match manifest") &&
             ok;
        return;
      }
    }
    std::cerr << "EXPECT FAILED: missing generated helper op " << name << '\n';
    ok = false;
  };

  check("Hello", exv::helper::HelperOp::Hello, false);
  check("StartSession", exv::helper::HelperOp::StartSession, false);
  check("PrepareTunnelDevice", exv::helper::HelperOp::PrepareTunnelDevice, true);
  check("ApplyTunnelConfig", exv::helper::HelperOp::ApplyTunnelConfig, true);
  check("Heartbeat", exv::helper::HelperOp::Heartbeat, true);
  check("Cleanup", exv::helper::HelperOp::Cleanup, true);
  check("GetSnapshot", exv::helper::HelperOp::GetSnapshot, false);
  check("Shutdown", exv::helper::HelperOp::Shutdown, true);

  return ok ? 0 : 1;
}

int test_hello_has_no_version_fields() {
  bool ok = true;

  exv::helper::HelloRequest req;
  json request_json = req;
  ok = expect(!json_has_key_recursive(request_json, "protocol_version"),
              "HelloRequest must not contain protocol_version") &&
       ok;
  ok = expect(!json_has_key_recursive(request_json, "client_version"),
              "HelloRequest must not contain client_version") &&
       ok;

  exv::helper::HelloResponse resp;
  resp.capabilities = {"tunnel_device_create", "route_apply", "dns_apply"};
  resp.mode = exv::helper::HelperMode::Transient;
  resp.startup_context.launch_mode = "oneshot";
  resp.startup_context.parent_pid = 1234;
  resp.session_state.active = false;

  json response_json = resp;
  ok = expect(response_json.contains("capabilities"),
              "HelloResponse must include capabilities") &&
       ok;
  ok = expect(response_json.contains("mode"), "HelloResponse must include mode") &&
       ok;
  ok = expect(response_json.contains("startup_context"),
              "HelloResponse must include startup_context") &&
       ok;
  ok = expect(response_json.contains("session_state"),
              "HelloResponse must include session_state") &&
       ok;
  ok = expect(!json_has_key_recursive(response_json, "protocol_version"),
              "HelloResponse must not contain protocol_version") &&
       ok;
  ok = expect(!json_has_key_recursive(response_json, "server_version"),
              "HelloResponse must not contain server_version") &&
       ok;

  return ok ? 0 : 1;
}

int test_hello_mode_matches_startup_context() {
  bool ok = true;
  exv::helper::HelperHandler service_handler;

  exv::helper::HelperStartupContext service_context;
  service_context.launch_mode = "service";
  service_handler.set_startup_context(service_context);

  auto service_hello =
      dispatch_json(service_handler, exv::helper::HelperOp::Hello, json::object());
  ok = expect(service_hello.success, "service Hello should succeed") && ok;
  auto service_payload =
      exv::helper::hello_response_from_json(json::parse(service_hello.payload_json));
  ok = expect(service_payload.mode == exv::helper::HelperMode::Resident,
              "service Hello should report Resident mode") &&
       ok;

  exv::helper::HelperHandler oneshot_handler;
  exv::helper::HelperStartupContext oneshot_context;
  oneshot_context.launch_mode = "oneshot";
  oneshot_handler.set_startup_context(oneshot_context);

  auto oneshot_hello =
      dispatch_json(oneshot_handler, exv::helper::HelperOp::Hello, json::object());
  ok = expect(oneshot_hello.success, "oneshot Hello should succeed") && ok;
  auto oneshot_payload =
      exv::helper::hello_response_from_json(json::parse(oneshot_hello.payload_json));
  ok = expect(oneshot_payload.mode == exv::helper::HelperMode::Transient,
              "oneshot Hello should report Transient mode") &&
       ok;

  return ok ? 0 : 1;
}

int test_start_session_rejects_second_active_session() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::StartSessionRequest first_req;
  first_req.profile_id.value = "profile-a";
  auto first = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(first_req));
  ok = expect(first.success, "first StartSession should succeed") && ok;

  exv::helper::StartSessionRequest second_req;
  second_req.profile_id.value = "profile-b";
  auto second = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                              json(second_req));
  ok = expect(!second.success, "second StartSession must be rejected") && ok;
  ok = expect(second.error_code == "session_conflict",
              "duplicate StartSession must return session_conflict") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "handler must keep only one active session") &&
       ok;

  return ok ? 0 : 1;
}

int test_shutdown_cleans_active_session() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req));
  ok = expect(start.success, "StartSession should succeed") && ok;
  const auto start_payload = json::parse(start.payload_json);
  const auto start_resp =
      exv::helper::start_session_response_from_json(start_payload);

  exv::helper::ShutdownRequest shutdown_req;
  shutdown_req.session_id = start_resp.session_id;
  shutdown_req.policy.remove_adapter = true;
  auto shutdown = dispatch_json(handler, exv::helper::HelperOp::Shutdown,
                                json(shutdown_req));
  ok = expect(shutdown.success, "Shutdown should succeed") && ok;
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "Shutdown must remove the active session") &&
       ok;

  const auto shutdown_resp =
      exv::helper::shutdown_response_from_json(json::parse(shutdown.payload_json));
  ok = expect(shutdown_resp.cleanup_success,
              "Shutdown response must report cleanup success") &&
       ok;

  return ok ? 0 : 1;
}

int test_cleanup_retains_resources_when_platform_cleanup_unavailable() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req));
  ok = expect(start.success, "StartSession should succeed before cleanup test") &&
       ok;
  const auto start_resp =
      exv::helper::start_session_response_from_json(json::parse(start.payload_json));

  handler.cleanup_registry().add_resource(start_resp.session_id,
                                          {"route", "0.0.0.0/0"});

  exv::helper::CleanupRequest cleanup_req;
  cleanup_req.session_id = start_resp.session_id;
  auto cleanup = dispatch_json(handler, exv::helper::HelperOp::Cleanup,
                               json(cleanup_req));
  ok = expect(!cleanup.success,
              "Cleanup must fail when managed resources cannot be cleaned") &&
       ok;
  ok = expect(cleanup.error_code == "cleanup_partial",
              "Cleanup failure should report cleanup_partial") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "failed cleanup must keep the active session for retry") &&
       ok;
  ok = expect(handler.cleanup_registry().all_records().size() == 1,
              "failed cleanup must keep registry records for retry") &&
       ok;

  return ok ? 0 : 1;
}

int test_network_ops_do_not_report_fake_success() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req));
  ok = expect(start.success, "StartSession should succeed") && ok;
  const auto start_resp =
      exv::helper::start_session_response_from_json(json::parse(start.payload_json));

  exv::helper::PrepareTunnelDeviceRequest prepare_req;
  prepare_req.session_id = start_resp.session_id;
  prepare_req.adapter_name = "ECNU-VPN";
  auto prepare = dispatch_json(handler, exv::helper::HelperOp::PrepareTunnelDevice,
                               json(prepare_req));
  ok = expect(!prepare.success,
              "PrepareTunnelDevice must not fake platform success") &&
       ok;
  ok = expect(prepare.error_code == "network_ops_unavailable",
              "PrepareTunnelDevice must report network_ops_unavailable") &&
       ok;

  exv::helper::ApplyTunnelConfigRequest apply_req;
  apply_req.config.session_id = start_resp.session_id;
  apply_req.config.interface_address = "10.0.0.2/24";
  auto apply = dispatch_json(handler, exv::helper::HelperOp::ApplyTunnelConfig,
                             json(apply_req));
  ok = expect(!apply.success,
              "ApplyTunnelConfig must not fake platform success") &&
       ok;
  ok = expect(apply.error_code == "network_ops_unavailable",
              "ApplyTunnelConfig must report network_ops_unavailable") &&
       ok;

  return ok ? 0 : 1;
}

int test_oneshot_heartbeat_timeout_cleans_and_requests_exit() {
  bool ok = true;
  exv::helper::LeaseTimeoutConfig config;
  config.transient_heartbeat_timeout = std::chrono::seconds(0);
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy(config)};

  exv::helper::HelperStartupContext context;
  context.launch_mode = "oneshot";
  handler.set_startup_context(context);

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req));
  ok = expect(start.success, "oneshot StartSession should succeed") && ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "oneshot should have an active session before timeout") &&
       ok;

  handler.tick();
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "oneshot timeout tick must clean the active session") &&
       ok;
  ok = expect(handler.should_stop(),
              "oneshot timeout tick must request helper process exit") &&
       ok;

  return ok ? 0 : 1;
}

int test_service_heartbeat_timeout_cleans_without_exit() {
  bool ok = true;
  exv::helper::LeaseTimeoutConfig config;
  config.transient_heartbeat_timeout = std::chrono::seconds(0);
  exv::helper::HelperHandler handler{
      exv::helper::HelperLifecyclePolicy(config)};

  exv::helper::HelperStartupContext context;
  context.launch_mode = "service";
  handler.set_startup_context(context);

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                             json(start_req));
  ok = expect(start.success, "service StartSession should succeed") && ok;

  handler.tick();
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "service timeout tick must clean the active session") &&
       ok;
  ok = expect(!handler.should_stop(),
              "service timeout tick must keep daemon running") &&
       ok;

  return ok ? 0 : 1;
}

int test_helper_message_fields_are_credential_free() {
  bool ok = true;

  auto check = [&](const json &message, const char *name) {
    if (json_contains_credential_field(message)) {
      std::cerr << "EXPECT FAILED: " << name
                << " contains a credential-like field\n";
      ok = false;
    }
  };

  check(json(exv::helper::HelloRequest{}), "HelloRequest");
  check(json(exv::helper::HelloResponse{}), "HelloResponse");

  exv::helper::StartSessionRequest start;
  start.profile_id.value = "profile";
  check(json(start), "StartSessionRequest");

  exv::helper::PrepareTunnelDeviceRequest prepare;
  prepare.session_id.value = "ses-1";
  prepare.adapter_name = "adapter";
  check(json(prepare), "PrepareTunnelDeviceRequest");

  exv::helper::ApplyTunnelConfigRequest apply;
  apply.config.session_id.value = "ses-1";
  apply.config.interface_address = "10.0.0.2/24";
  apply.config.routes.push_back({"0.0.0.0/0", "10.0.0.1", 100});
  apply.config.dns.servers = {"8.8.8.8"};
  check(json(apply), "ApplyTunnelConfigRequest");

  exv::helper::HeartbeatRequest heartbeat;
  heartbeat.session_id.value = "ses-1";
  heartbeat.core_phase = "Connected";
  check(json(heartbeat), "HeartbeatRequest");

  exv::helper::CleanupRequest cleanup;
  cleanup.session_id.value = "ses-1";
  check(json(cleanup), "CleanupRequest");

  check(json(exv::helper::GetSnapshotRequest{}), "GetSnapshotRequest");

  exv::helper::ShutdownRequest shutdown;
  shutdown.session_id.value = "ses-1";
  check(json(shutdown), "ShutdownRequest");

  return ok ? 0 : 1;
}

} // namespace

int main() {
  int failures = 0;

  std::cout << "=== Helper Contract Tests ===\n";
  failures += test_manifest_declares_single_helper_protocol();
  failures += test_generated_contract_matches_helper_manifest();
  failures += test_hello_has_no_version_fields();
  failures += test_hello_mode_matches_startup_context();
  failures += test_start_session_rejects_second_active_session();
  failures += test_shutdown_cleans_active_session();
  failures += test_cleanup_retains_resources_when_platform_cleanup_unavailable();
  failures += test_network_ops_do_not_report_fake_success();
  failures += test_oneshot_heartbeat_timeout_cleans_and_requests_exit();
  failures += test_service_heartbeat_timeout_cleans_without_exit();
  failures += test_helper_message_fields_are_credential_free();

  if (failures == 0) {
    std::cout << "helper_contract_test: all tests passed\n";
  } else {
    std::cerr << "helper_contract_test: " << failures << " test(s) FAILED\n";
  }
  return failures == 0 ? 0 : 1;
}
