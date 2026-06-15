// Tests for RouteActions as persisted config route adapters.

#include "core/config/config.hpp"
#include "core/config/config_manager.hpp"
#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/route_actions.hpp"

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

struct RouteActionsFixture {
  std::string config_dir = unique_temp_dir("exv-route-actions-test");
  exv::core_api::AppRpcDispatcher dispatcher;
  exv::core_api::RouteActions route{config_dir};

  RouteActionsFixture() {
    ecnuvpn::config::ConfigManager manager(config_dir);
    ecnuvpn::Config cfg;
    cfg.routes.clear();
    manager.save(cfg);
    route.register_handlers(dispatcher);
  }

  exv::core_api::RpcResponse dispatch(const std::string &action,
                                      const std::string &payload = "{}") {
    exv::core_api::RpcRequest req;
    req.action = action;
    req.payload_json = payload;
    req.request_id = "test-req";
    return dispatcher.dispatch(req);
  }
};

} // namespace

int main() {
  bool ok = true;

  {
    RouteActionsFixture fix;
    auto resp = fix.dispatch("route.list");
    ok = expect(resp.success, "route.list should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload.contains("routes"), "response missing routes") && ok;
    ok = expect(payload["routes"].is_array(), "routes should be an array") &&
         ok;
    ok = expect(payload["routes"].empty(), "routes should start empty") && ok;
  }

  {
    RouteActionsFixture fix;
    auto resp =
        fix.dispatch("route.add", R"({"destination":"10.0.0.0/8"})");
    ok = expect(resp.success, "route.add should persist a route") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["added"] == true, "added should be true") && ok;

    auto list_resp = fix.dispatch("route.list");
    auto list_payload = json::parse(list_resp.payload_json);
    ok = expect(list_payload["routes"].size() == 1,
                "routes should have 1 entry after add") &&
         ok;
    ok = expect(list_payload["routes"][0]["cidr"] == "10.0.0.0/8",
                "persisted route cidr should match") &&
         ok;
    ok = expect(list_payload["routes"][0]["destination"] == "10.0.0.0/8",
                "legacy destination field should mirror cidr") &&
         ok;
  }

  {
    RouteActionsFixture fix;
    auto resp = fix.dispatch("route.add", R"({"gateway":"10.0.0.1"})");
    ok = expect(!resp.success, "add with missing route should fail") && ok;
    ok = expect(resp.error_code == "invalid_payload",
                "error code should be invalid_payload") &&
         ok;
  }

  {
    RouteActionsFixture fix;
    fix.dispatch("route.add", R"({"destination":"172.16.0.0/12"})");

    auto resp =
        fix.dispatch("route.remove", R"({"destination":"172.16.0.0/12"})");
    ok = expect(resp.success, "route.remove should succeed") && ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["removed"] == true, "removed should be true") && ok;

    auto list_resp = fix.dispatch("route.list");
    auto list_payload = json::parse(list_resp.payload_json);
    ok = expect(list_payload["routes"].empty(),
                "routes should be empty after remove") &&
         ok;
  }

  {
    RouteActionsFixture fix;
    auto resp =
        fix.dispatch("route.remove", R"({"destination":"192.168.99.0/24"})");
    ok = expect(!resp.success, "remove nonexistent route should fail") && ok;
    ok = expect(resp.error_code == "not_found",
                "error code should be not_found") &&
         ok;
    ok = expect(resp.error_message.find("192.168.99.0/24") !=
                    std::string::npos,
                "error message should mention the destination") &&
         ok;
  }

  {
    RouteActionsFixture fix;
    auto resp =
        fix.dispatch("route.enable", R"({"destination":"10.1.0.0/16"})");
    ok = expect(!resp.success,
                "route.enable should be explicit unsupported behavior") &&
         ok;
    ok = expect(resp.error_code == "unsupported_action",
                "route.enable should return unsupported_action") &&
         ok;
  }

  {
    RouteActionsFixture fix;
    auto resp =
        fix.dispatch("route.disable", R"({"destination":"10.2.0.0/16"})");
    ok = expect(!resp.success,
                "route.disable should be explicit unsupported behavior") &&
         ok;
    ok = expect(resp.error_code == "unsupported_action",
                "route.disable should return unsupported_action") &&
         ok;
  }

  {
    RouteActionsFixture fix;
    fix.dispatch("routes.add", R"({"cidr":"10.10.0.0/16"})");
    fix.dispatch("routes.add", R"({"cidr":"10.20.0.0/16"})");
    fix.dispatch("routes.add", R"({"cidr":"10.30.0.0/16"})");

    auto list_resp = fix.dispatch("routes.list");
    auto list_payload = json::parse(list_resp.payload_json);
    ok = expect(list_payload["routes"].size() == 3,
                "routes.list should see persisted routes") &&
         ok;

    fix.dispatch("routes.remove", R"({"cidr":"10.20.0.0/16"})");

    list_resp = fix.dispatch("routes.list");
    list_payload = json::parse(list_resp.payload_json);
    ok = expect(list_payload["routes"].size() == 2,
                "routes should have 2 entries after removal") &&
         ok;
  }

  {
    RouteActionsFixture fix;
    auto resp = fix.dispatch("routes.reset");
    ok = expect(resp.success, "routes.reset should succeed") && ok;
    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["routes"].is_array() && !payload["routes"].empty(),
                "routes.reset should restore default config routes") &&
         ok;
  }

  {
    RouteActionsFixture fix;
    exv::core_api::RpcRequest req;
    req.action = "route.list";
    req.payload_json = "{}";
    req.request_id = "trace-route-55";
    auto resp = fix.dispatcher.dispatch(req);
    ok = expect(resp.request_id == "trace-route-55",
                "request_id should be propagated") &&
         ok;
  }

  if (ok) {
    std::cout << "route_actions_test: all assertions passed\n";
  } else {
    std::cerr << "route_actions_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
