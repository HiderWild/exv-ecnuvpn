// Native-only helper session contract tests.

#include "helper/common/helper_messages.hpp"
#include "helper/helper_handler.hpp"
#include "helper/helper_network_ops.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <memory>
#include <string>
#include <vector>

using json = nlohmann::json;

namespace exv::logger {
void info(const std::string &) {}
} // namespace exv::logger

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << '\n';
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

exv::helper::AcquireCoreLeaseResponse
acquire_core_lease(exv::helper::HelperHandler &handler, int core_pid = 4321) {
  exv::helper::AcquireCoreLeaseRequest acquire_req;
  acquire_req.core_pid = core_pid;
  acquire_req.purpose = "connect";

  const auto acquired =
      dispatch_json(handler, exv::helper::HelperOp::AcquireCoreLease,
                    json(acquire_req));
  if (!acquired.success) {
    return {};
  }
  return exv::helper::acquire_core_lease_response_from_json(
      json::parse(acquired.payload_json));
}

exv::helper::StartSessionResponse
start_session_with_core_lease(exv::helper::HelperHandler &handler) {
  const auto acquired = acquire_core_lease(handler);
  if (!acquired.accepted) {
    return {};
  }

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  const auto start =
      dispatch_json(handler, exv::helper::HelperOp::StartSession,
                    json(start_req));
  if (!start.success) {
    return {};
  }
  return exv::helper::start_session_response_from_json(
      json::parse(start.payload_json));
}

class RecordingHelperNetworkOps final : public exv::helper::HelperNetworkOps {
public:
  exv::helper::PrepareTunnelDeviceResponse prepare_tunnel_device(
      const exv::helper::PrepareTunnelDeviceRequest &request,
      std::vector<exv::helper::ManagedResource> *created_resources) override {
    ++prepare_count;
    last_prepare_session = request.session_id.value;
    created_resources->push_back({"adapter", request.adapter_name});

    exv::helper::PrepareTunnelDeviceResponse response;
    response.device_path = "helper-device://" + request.adapter_name;
    response.mtu = 1280;
    return response;
  }

  exv::helper::ApplyTunnelConfigResponse apply_tunnel_config(
      const exv::helper::ApplyTunnelConfigRequest &request,
      std::vector<exv::helper::ManagedResource> *created_resources) override {
    ++apply_count;
    last_apply_session = request.config.session_id.value;
    for (const auto &route : request.config.routes) {
      created_resources->push_back({"route", route.destination});
    }
    for (const auto &server : request.config.dns.servers) {
      created_resources->push_back({"dns", server});
    }

    exv::helper::ApplyTunnelConfigResponse response;
    response.success = true;
    return response;
  }

  exv::helper::CleanupResponse cleanup(
      const exv::helper::SessionId &session_id,
      const exv::helper::CleanupPolicy &policy,
      const std::vector<exv::helper::ManagedResource> &resources) override {
    (void)policy;
    ++cleanup_count;
    last_cleanup_session = session_id.value;
    last_cleanup_resource_count = resources.size();

    exv::helper::CleanupResponse response;
    response.success = true;
    return response;
  }

  int prepare_count = 0;
  int apply_count = 0;
  int cleanup_count = 0;
  std::size_t last_cleanup_resource_count = 0;
  std::string last_prepare_session;
  std::string last_apply_session;
  std::string last_cleanup_session;
};

bool test_start_session_requires_core_lease() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  const auto start =
      dispatch_json(handler, exv::helper::HelperOp::StartSession,
                    json(start_req));

  ok = expect(!start.success,
              "StartSession before AcquireCoreLease must be rejected") &&
       ok;
  ok = expect(start.error_code == "core_lease_required",
              "StartSession without core lease must return core_lease_required") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "rejected StartSession must not create a helper session") &&
       ok;

  return ok;
}

bool test_start_session_rejects_deleted_supervisor_payloads() {
  bool ok = true;

  for (const std::string &mode : {"password", "auth_session"}) {
    exv::helper::HelperHandler handler;
    const auto acquired = acquire_core_lease(handler);
    ok = expect(acquired.accepted,
                "core lease should be acquired before deleted-start rejection") &&
         ok;

    const json payload = {
        {"profile_id", "profile-a"},
        {"native_start_mode", mode},
    };
    const auto start =
        dispatch_json(handler, exv::helper::HelperOp::StartSession, payload);

    ok = expect(!start.success,
                "StartSession must reject deleted native start payloads") &&
         ok;
    ok = expect(start.error_code == "supervisor_removed",
                "deleted start payload must return supervisor_removed") &&
         ok;
    ok = expect(handler.lease_manager().active_session_count() == 0,
                "rejected deleted start payload must not create a helper session") &&
         ok;
  }

  return ok;
}

bool test_core_lease_allows_helper_broker_session_only() {
  bool ok = true;
  exv::helper::HelperHandler handler;

  const auto acquired = acquire_core_lease(handler);
  ok = expect(acquired.accepted, "valid AcquireCoreLease should be accepted") &&
       ok;
  ok = expect(!acquired.lease_id.empty(),
              "accepted AcquireCoreLease should return a lease_id") &&
       ok;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "profile-a";
  const auto start =
      dispatch_json(handler, exv::helper::HelperOp::StartSession,
                    json(start_req));

  ok = expect(start.success,
              "StartSession after AcquireCoreLease should create a broker session") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 1,
              "helper must track exactly one active broker session") &&
       ok;

  return ok;
}

bool test_helper_delegates_network_ops_and_owns_cleanup() {
  bool ok = true;
  auto network_ops = std::make_shared<RecordingHelperNetworkOps>();
  exv::helper::HelperHandler handler{exv::helper::HelperLifecyclePolicy{},
                                     network_ops};

  const auto start_resp = start_session_with_core_lease(handler);
  ok = expect(!start_resp.session_id.value.empty(),
              "StartSession should produce a helper session id") &&
       ok;

  exv::helper::PrepareTunnelDeviceRequest prepare_req;
  prepare_req.session_id = start_resp.session_id;
  prepare_req.adapter_name = "EXV";
  const auto prepare =
      dispatch_json(handler, exv::helper::HelperOp::PrepareTunnelDevice,
                    json(prepare_req));
  ok = expect(prepare.success,
              "PrepareTunnelDevice should be delegated to network ops") &&
       ok;
  const auto prepare_resp =
      exv::helper::prepare_tunnel_device_response_from_json(
          json::parse(prepare.payload_json));
  ok = expect(prepare_resp.device_path == "helper-device://EXV",
              "PrepareTunnelDevice should return the delegated device path") &&
       ok;

  exv::helper::ApplyTunnelConfigRequest apply_req;
  apply_req.config.session_id = start_resp.session_id;
  apply_req.config.interface_address = "10.0.0.2/24";
  apply_req.config.routes.push_back({"10.0.0.0/8", "10.0.0.1", 10});
  apply_req.config.dns.servers = {"10.0.0.53"};
  const auto apply =
      dispatch_json(handler, exv::helper::HelperOp::ApplyTunnelConfig,
                    json(apply_req));
  ok = expect(apply.success,
              "ApplyTunnelConfig should be delegated to network ops") &&
       ok;

  const auto resources =
      handler.cleanup_registry().get_resources(start_resp.session_id);
  ok = expect(resources.size() == 3,
              "helper should register adapter, route, and DNS resources") &&
       ok;

  exv::helper::CleanupRequest cleanup_req;
  cleanup_req.session_id = start_resp.session_id;
  const auto cleanup =
      dispatch_json(handler, exv::helper::HelperOp::Cleanup,
                    json(cleanup_req));
  ok = expect(cleanup.success,
              "Cleanup should be delegated to network ops") &&
       ok;
  ok = expect(handler.lease_manager().active_session_count() == 0,
              "successful cleanup should remove the helper session") &&
       ok;
  ok = expect(handler.cleanup_registry().all_records().empty(),
              "successful cleanup should remove tracked resources") &&
       ok;
  ok = expect(network_ops->prepare_count == 1,
              "network ops prepare should be called once") &&
       ok;
  ok = expect(network_ops->apply_count == 1,
              "network ops apply should be called once") &&
       ok;
  ok = expect(network_ops->cleanup_count == 1,
              "network ops cleanup should be called once") &&
       ok;
  ok = expect(network_ops->last_cleanup_resource_count == 3,
              "network ops cleanup should receive all tracked resources") &&
       ok;

  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = test_start_session_requires_core_lease() && ok;
  ok = test_start_session_rejects_deleted_supervisor_payloads() && ok;
  ok = test_core_lease_allows_helper_broker_session_only() && ok;
  ok = test_helper_delegates_network_ops_and_owns_cleanup() && ok;

  if (ok) {
    std::cout << "native_helper_session_test: all tests passed\n";
  } else {
    std::cerr << "native_helper_session_test: FAILED\n";
  }
  return ok ? 0 : 1;
}
