// Tests for ServiceActions as platform status adapters.

#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/service_actions.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/tunnel_controller/tunnel_intent.hpp"
#include "../support/fake_helper.hpp"
#include "../support/fake_platform_network_ops.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
#include <iostream>
#include <string>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::string unique_temp_dir(const char *name) {
  const auto now = std::chrono::steady_clock::now().time_since_epoch().count();
  auto dir = std::filesystem::temp_directory_path() /
             (std::string(name) + "-" + std::to_string(now));
  std::filesystem::create_directories(dir);
  return dir.string();
}

struct ServiceActionsFixture {
  std::string config_dir = unique_temp_dir("exv-service-actions-test");
  exv::core_api::AppRpcDispatcher dispatcher;
  exv::core_api::ServiceActions service{config_dir};

  ServiceActionsFixture() { service.register_handlers(dispatcher); }

  exv::core_api::RpcResponse dispatch(const std::string &action,
                                      const std::string &payload = "{}") {
    exv::core_api::RpcRequest req;
    req.action = action;
    req.payload_json = payload;
    req.request_id = "test-req";
    return dispatcher.dispatch(req);
  }
};

exv::core::UserIntent make_active_intent() {
  exv::core::UserIntent intent;
  intent.desired_connected = true;
  intent.auto_reconnect = true;
  intent.profile_id.value = "service-actions-active";
  return intent;
}

} // namespace

int main() {
  bool ok = true;

  {
    ServiceActionsFixture fix;
    auto resp = fix.dispatch("service.helper_status");
    ok = expect(resp.success, "helper_status should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("resolved"),
                "helper status should include backend resolution state") &&
         ok;
    ok = expect(payload.value("status", std::string()) != "unknown",
                "helper status must not be a fake unknown status") &&
         ok;
  }

  {
    ServiceActionsFixture fix;
    auto resp = fix.dispatch("service.status");
    ok = expect(resp.success, "service.status should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("installed"),
                "service.status should include installed") &&
         ok;
    ok = expect(payload.contains("running"),
                "service.status should include running") &&
         ok;
  }

  {
    auto helper = std::make_shared<exv::test::FakeHelper>();
    helper->connect();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    auto controller =
        std::make_shared<exv::core::TunnelController>(helper, net_ops);
    controller->connect(make_active_intent());

    exv::core_api::AppRpcDispatcher dispatcher;
    exv::core_api::ServiceActions service(
        unique_temp_dir("exv-service-active-install-test"), controller);
    service.register_handlers(dispatcher);

    exv::core_api::RpcRequest req;
    req.action = "service.install";
    req.payload_json = "{}";
    req.request_id = "active-install";
    auto resp = dispatcher.dispatch(req);
    ok = expect(!resp.success,
                "service.install should use the active helper IPC path") &&
         ok;
    ok = expect(resp.error_code == "service_install_failed",
                "service.install active helper failure should not fall back to platform service manager") &&
         ok;
    ok = expect(resp.error_code != "helper_unavailable",
                "service.install should not ignore the current helper client") &&
         ok;
  }

  {
    ServiceActionsFixture fix;
    auto resp = fix.dispatch("service.uninstall");
    ok = expect(!resp.success,
                "service.uninstall should fail when no helper instance is available") &&
         ok;
    ok = expect(resp.error_code == "service_not_installed" ||
                    resp.error_code == "service_installed_not_running" ||
                    resp.error_code == "helper_unavailable",
                "service.uninstall should route through helper/backend resolution before privileged maintenance") &&
         ok;
    ok = expect(!resp.error_message.empty(),
                "error message should not be empty") &&
         ok;
  }

  {
    auto helper = std::make_shared<exv::test::FakeHelper>();
    auto net_ops = std::make_shared<exv::test::FakePlatformNetworkOps>();
    auto controller =
        std::make_shared<exv::core::TunnelController>(helper, net_ops);
    controller->connect(make_active_intent());

    exv::core_api::AppRpcDispatcher dispatcher;
    exv::core_api::ServiceActions service(
        unique_temp_dir("exv-service-active-test"), controller);
    service.register_handlers(dispatcher);

    exv::core_api::RpcRequest req;
    req.action = "service.uninstall";
    req.payload_json = "{}";
    req.request_id = "active-uninstall";
    auto resp = dispatcher.dispatch(req);
    ok = expect(!resp.success,
                "service.uninstall should reject active VPN sessions") &&
         ok;
    ok = expect(resp.error_code == "vpn_session_active",
                "service.uninstall active rejection should use vpn_session_active") &&
         ok;
  }

  {
    ServiceActionsFixture fix;
    auto resp = fix.dispatch("service.driver_status");
    ok = expect(resp.success, "driver_status should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("effective_driver_status") ||
                    payload.contains("supported"),
                "driver_status should expose platform driver status") &&
         ok;
    ok = expect(payload.value("status", std::string()) != "unknown",
                "driver_status must not be a fake unknown status") &&
         ok;
  }

  {
    ServiceActionsFixture fix;
    exv::core_api::RpcRequest req;
    req.action = "service.helper_status";
    req.payload_json = "{}";
    req.request_id = "trace-svc-77";
    auto resp = fix.dispatcher.dispatch(req);
    ok = expect(resp.request_id == "trace-svc-77",
                "request_id should be propagated") &&
         ok;
  }

  if (ok) {
    std::cout << "service_actions_test: all assertions passed\n";
  } else {
    std::cerr << "service_actions_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
