// Tests for ServiceActions as platform status adapters.

#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/service_actions.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/tunnel_controller/tunnel_intent.hpp"
#include "core/use_cases/system_status_use_cases.hpp"
#include "../support/fake_helper.hpp"
#include "../support/fake_platform_network_ops.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>

using json = nlohmann::json;

namespace {

#ifndef EXV_SOURCE_DIR
#define EXV_SOURCE_DIR "."
#endif

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

std::string read_repo_text(const std::filesystem::path &relative_path) {
  const auto source_file = std::filesystem::path(__FILE__);
  const std::filesystem::path candidates[] = {
      std::filesystem::path(EXV_SOURCE_DIR) / relative_path,
      source_file.is_absolute()
          ? source_file.parent_path().parent_path().parent_path() /
                relative_path
          : std::filesystem::path{},
      std::filesystem::current_path() / relative_path,
      std::filesystem::current_path().parent_path() / relative_path,
  };
  std::ifstream input;
  for (const auto &candidate : candidates) {
    input.open(candidate);
    if (input.is_open()) {
      break;
    }
    input.clear();
  }
  std::ostringstream buffer;
  buffer << input.rdbuf();
  return buffer.str();
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
    if (resp.success) {
      auto payload = json::parse(resp.payload_json);
      ok = expect(payload.contains("service_status") &&
                      !payload["service_status"].value("installed", true),
                  "service.uninstall may succeed only when latest status reports not installed") &&
           ok;
    } else {
      ok = expect(resp.error_code == "service_not_installed" ||
                      resp.error_code == "service_installed_not_running" ||
                      resp.error_code == "helper_unavailable" ||
                      resp.error_code == "service_uninstall_failed" ||
                      resp.error_code == "service_start_failed" ||
                      resp.error_code == "oneshot_elevation_denied" ||
                      resp.error_code == "helper_rpc_failed" ||
                      resp.error_code == "oneshot_not_supported",
                  "service.uninstall should route through helper/backend resolution before privileged maintenance") &&
           ok;
      ok = expect(!resp.error_message.empty(),
                  "error message should not be empty") &&
           ok;
    }
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
    exv::helper::UninstallServiceResponse operation;
    operation.success = true;
    operation.exit_code = 0;
    operation.message = "deleted";

    json still_installed = {{"installed", true},
                            {"running", false},
                            {"service_state", 1}};
    auto pending =
        exv::core::finalize_service_uninstall_result(operation, still_installed);
    ok = expect(!pending.success,
                "service uninstall should fail if latest status still reports installed") &&
         ok;
    ok = expect(pending.error_code == "service_uninstall_failed",
                "still-installed uninstall should use service_uninstall_failed") &&
         ok;
    ok = expect(pending.payload.contains("service_status") &&
                    pending.payload["service_status"].value("installed", false),
                "failed service uninstall should include latest service_status payload") &&
         ok;
    ok = expect(pending.payload.contains("operation") &&
                    pending.payload["operation"].value("success", false),
                "failed service uninstall should preserve helper operation payload") &&
         ok;

    json removed = still_installed;
    removed["installed"] = false;
    auto completed =
        exv::core::finalize_service_uninstall_result(operation, removed);
    ok = expect(completed.success,
                "service uninstall should succeed when latest status reports not installed") &&
         ok;
    ok = expect(completed.payload["service_status"].value("installed", true) ==
                    false,
                "successful service uninstall should include not-installed status") &&
         ok;
  }

  {
    const auto use_cases_source =
        read_repo_text("src/core/use_cases/system_status_use_cases.cpp");
    const auto desktop_source =
        read_repo_text("src/core/app_api/desktop_system_actions.cpp");
    ok = expect(use_cases_source.find("return finalize_service_uninstall_result(") !=
                    std::string::npos,
                "core service.uninstall should finalize success against latest service_status") &&
         ok;
    ok = expect(desktop_source.find(
                    "auto finalized = exv::core::finalize_service_uninstall_result(") !=
                    std::string::npos,
                "desktop service.uninstall should inspect current-helper "
                "success against latest service_status") &&
         ok;
    ok = expect(desktop_source.find("if (finalized.success)") !=
                    std::string::npos,
                "desktop service.uninstall should fall back when self-delete "
                "has not removed the service yet") &&
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
