// Tests for MaintenanceActions: inspectCore, killStaleCore.
//
// 5.1: maintenance.inspectCore returns registry-derived state and risk level
// 5.2: maintenance.killStaleCore without confirm returns confirmation_required
// 5.3: Confirmed kill prefers IPC shutdown when communication is possible
// 5.4: Confirmed kill by PID verifies process dead, lock released, IPC unavailable
// 5.5: Unknown registry state is reported as unknown risk

#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/maintenance_actions.hpp"
#include "core/lifecycle/core_registry.hpp"
#include "core/lifecycle/core_paths.hpp"
#include "core/lifecycle/core_lock.hpp"
#include "contracts/generated/system_contract.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
#include <fstream>
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

#ifdef _WIN32
#include <windows.h>
int current_process_pid() {
  return static_cast<int>(GetCurrentProcessId());
}
#else
#include <unistd.h>
int current_process_pid() {
  return static_cast<int>(getpid());
}
#endif

struct MaintenanceFixture {
  std::string state_dir = unique_temp_dir("exv-maint-test");
  exv::core_api::AppRpcDispatcher dispatcher;
  exv::core_api::MaintenanceProcessControl control;
  std::shared_ptr<exv::core_api::MaintenanceActions> actions;

  bool ipc_shutdown_called = false;
  bool terminate_called = false;
  bool release_lock_called = false;
  int terminated_pid = 0;

  MaintenanceFixture() {
    control.is_pid_alive = [this](int pid) -> bool {
      return pid == current_process_pid();
    };
    control.terminate_pid = [this](int pid) -> bool {
      terminate_called = true;
      terminated_pid = pid;
      return true;
    };
    control.try_ipc_shutdown = [this](const std::string &) -> bool {
      ipc_shutdown_called = true;
      return true;
    };
    control.release_lock = [this](const std::string &) -> bool {
      release_lock_called = true;
      return true;
    };
    control.is_ipc_available = [this](const std::string &) -> bool {
      return false;
    };

    actions = std::make_shared<exv::core_api::MaintenanceActions>(
        state_dir, control);
    actions->register_handlers(dispatcher);
  }

  ~MaintenanceFixture() {
    std::error_code ec;
    std::filesystem::remove_all(state_dir, ec);
  }

  exv::core_api::RpcResponse dispatch(const std::string &action,
                                      const std::string &payload = "{}") {
    exv::core_api::RpcRequest req;
    req.action = action;
    req.payload_json = payload;
    req.request_id = "test-req";
    return dispatcher.dispatch(req);
  }

  void write_registry(const std::string &phase, bool connected,
                      bool network_ready, int pid = 0) {
    exv::core::lifecycle::CoreRegistrySnapshot snapshot;
    snapshot.core_instance_id = "core-maint-test";
    snapshot.pid = pid > 0 ? pid : current_process_pid();
    snapshot.core_path = "/usr/bin/exv";
    snapshot.ipc_path =
        exv::core::lifecycle::core_ipc_path(state_dir);
    snapshot.ipc_protocol_version = "ipc-v1";
    snapshot.app_version = "3.3.0";
    snapshot.contract_version =
        std::string(exv::contracts::generated::CONTRACT_VERSION);
    snapshot.started_at = "2026-06-16T12:00:00.000Z";
    snapshot.last_heartbeat_at = "2026-06-16T12:00:00.000Z";
    snapshot.last_known_tunnel_phase = phase;
    snapshot.last_known_connected = connected;
    snapshot.last_known_network_ready = network_ready;

    exv::core::lifecycle::write_core_registry(
        snapshot,
        exv::core::lifecycle::core_registry_path(state_dir));
  }
};

} // namespace

int main() {
  bool ok = true;

  // ── 5.1: maintenance.inspectCore returns registry-derived state and risk level ──
  {
    MaintenanceFixture fix;
    fix.write_registry("connected", true, true);

    auto resp = fix.dispatch("maintenance.inspectCore");
    ok = expect(resp.success,
                "5.1: inspectCore should succeed") &&
         ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["risk_level"] == "active",
                "5.1: connected registry should report active risk") &&
         ok;
    ok = expect(payload.contains("registry"),
                "5.1: response should contain registry") &&
         ok;
    ok = expect(payload["registry"]["last_known_tunnel_phase"] ==
                    "connected",
                "5.1: tunnel phase should be connected") &&
         ok;
    ok = expect(payload["registry"]["last_known_connected"] == true,
                "5.1: connected flag should be true") &&
         ok;
  }

  // ── 5.2: maintenance.killStaleCore without confirm returns confirmation_required ──
  {
    MaintenanceFixture fix;
    fix.write_registry("connected", true, true);

    auto resp = fix.dispatch("maintenance.killStaleCore");
    ok = expect(!resp.success,
                "5.2: killStaleCore without confirm should fail") &&
         ok;
    ok = expect(resp.error_code == "confirmation_required",
                "5.2: error code should be confirmation_required") &&
         ok;
  }

  // ── 5.3: Confirmed kill prefers IPC shutdown when communication is possible ──
  {
    MaintenanceFixture fix;
    // Create a new MaintenanceActions with our mock that returns is_ipc_available=true
    // We need to do this because MaintenanceActions takes the control by value
    auto ipc_available = true;
    auto& ctrl = fix.control;
    ctrl.is_ipc_available = [ipc_available](const std::string &) -> bool {
      return ipc_available;
    };
    exv::core_api::AppRpcDispatcher dispatcher;
    auto actions = std::make_shared<exv::core_api::MaintenanceActions>(
        fix.state_dir, ctrl);
    actions->register_handlers(dispatcher);

    fix.write_registry("connected", true, true);

    exv::core_api::RpcRequest req;
    req.action = "maintenance.killStaleCore";
    req.payload_json = R"({"confirm":true})";
    auto resp = dispatcher.dispatch(req);
    ok = expect(resp.success,
                "5.3: confirmed killStaleCore with IPC should succeed") &&
         ok;
    // Check that IPC shutdown was called via the lambda capture
    // We can't check fix.ipc_shutdown_called because we created a new actions object
    // Instead, just verify the shutdown_method in the response

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["shutdown_method"] == "ipc",
                "5.3: shutdown method should be ipc") &&
         ok;
  }

  // ── 5.4: Confirmed kill by PID verifies process dead, lock released, IPC unavailable ──
  {
    MaintenanceFixture fix;
    fix.control.is_ipc_available = [](const std::string &) -> bool {
      return false;
    };
    fix.write_registry("idle", false, false);

    auto resp = fix.dispatch("maintenance.killStaleCore",
                             R"({"confirm":true})");
    ok = expect(resp.success,
                "5.4: confirmed killStaleCore by PID should succeed") &&
         ok;
    ok = expect(fix.terminate_called,
                "5.4: should terminate by PID when IPC unavailable") &&
         ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["shutdown_method"] == "pid_terminate",
                "5.4: shutdown method should be pid_terminate") &&
         ok;
    ok = expect(payload.contains("verification"),
                "5.4: response should include verification") &&
         ok;
    ok = expect(fix.release_lock_called,
                "5.4: should release lock after termination") &&
         ok;
  }

  // ── Kill stale core with already-dead PID ──
  {
    MaintenanceFixture fix;
    fix.control.is_pid_alive = [](int) -> bool { return false; };
    fix.write_registry("idle", false, false, 9999);

    auto resp = fix.dispatch("maintenance.killStaleCore",
                             R"({"confirm":true})");
    ok = expect(resp.success,
                "5.4b: killStaleCore with dead PID should succeed") &&
         ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["shutdown_method"] == "already_dead",
                "5.4b: shutdown method should be already_dead") &&
         ok;
  }

  // ── 5.5: Unknown registry state is reported as unknown risk ──
  {
    MaintenanceFixture fix;
    {
      std::string reg_path =
          exv::core::lifecycle::core_registry_path(fix.state_dir);
      std::ofstream out(reg_path);
      out << "{corrupt json!!!";
    }

    auto resp = fix.dispatch("maintenance.inspectCore");
    ok = expect(resp.success,
                "5.5: inspectCore with corrupt registry should succeed") &&
         ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["risk_level"] == "unknown",
                "5.5: corrupt registry should report unknown risk") &&
         ok;
    ok = expect(payload["registry_state"] == "unknown",
                "5.5: registry_state should be unknown") &&
         ok;
  }

  // ── Missing registry returns none risk ──
  {
    MaintenanceFixture fix;

    auto resp = fix.dispatch("maintenance.inspectCore");
    ok = expect(resp.success,
                "5.5b: inspectCore with missing registry should succeed") &&
         ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["risk_level"] == "none",
                "5.5b: missing registry should report none risk") &&
         ok;
  }

  // ── Transition phase risk ──
  {
    MaintenanceFixture fix;
    fix.write_registry("connecting", false, false);

    auto resp = fix.dispatch("maintenance.inspectCore");
    ok = expect(resp.success,
                "5.5c: inspectCore with connecting phase should succeed") &&
         ok;

    auto payload = json::parse(resp.payload_json);
    ok = expect(payload["risk_level"] == "transition",
                "5.5c: connecting phase should report transition risk") &&
         ok;
  }

  // ── Contract verification ──
  {
    ok = expect(exv::contracts::generated::is_destructive_core_rpc_action(
                    "maintenance.killStaleCore"),
                "contract: maintenance.killStaleCore must be destructive") &&
         ok;
    ok = expect(exv::contracts::generated::is_core_rpc_action(
                    "maintenance.killStaleCore"),
                "contract: maintenance.killStaleCore must be core RPC") &&
         ok;
    ok = expect(exv::contracts::generated::is_core_rpc_action(
                    "maintenance.inspectCore"),
                "contract: maintenance.inspectCore must be core RPC") &&
         ok;
    ok = expect(exv::contracts::generated::is_standard_error_code(
                    "confirmation_required"),
                "contract: confirmation_required must be standard error") &&
         ok;
  }

  if (ok) {
    std::cout << "maintenance_actions_test: all assertions passed\n";
  } else {
    std::cerr << "maintenance_actions_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
