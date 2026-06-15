#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "core/vpn/vpn.hpp"
#include "vpn_engine/native_session_store.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>

namespace ecnuvpn {
Config g_runtime_test_config;

namespace config {
Config load() { return g_runtime_test_config; }
} // namespace config
} // namespace ecnuvpn

#ifdef _WIN32
#include <windows.h>
#else
#include <signal.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

namespace {

struct DummyProcess {
  int pid = -1;
#ifdef _WIN32
  HANDLE process = NULL;
  HANDLE thread = NULL;
#endif
};

int current_process_id() {
#ifdef _WIN32
  return static_cast<int>(GetCurrentProcessId());
#else
  return static_cast<int>(getpid());
#endif
}

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

DummyProcess spawn_dummy_process() {
  DummyProcess process;

#ifdef _WIN32
  STARTUPINFOA startup_info{};
  startup_info.cb = sizeof(startup_info);
  PROCESS_INFORMATION process_info{};
  char command_line[] = "cmd.exe /C ping -n 5 127.0.0.1 > nul";

  if (!CreateProcessA(nullptr, command_line, nullptr, nullptr, FALSE, 0,
                      nullptr, nullptr, &startup_info, &process_info)) {
    return process;
  }

  process.pid = static_cast<int>(process_info.dwProcessId);
  process.process = process_info.hProcess;
  process.thread = process_info.hThread;
#else
  pid_t pid = fork();
  if (pid == 0) {
    execlp("sleep", "sleep", "60", nullptr);
    _exit(127);
  }
  if (pid > 0) {
    process.pid = static_cast<int>(pid);
  }
#endif

  return process;
}

bool is_dummy_process_alive(const DummyProcess &process) {
  if (process.pid <= 0)
    return false;

#ifdef _WIN32
  if (!process.process)
    return false;

  DWORD exit_code = 0;
  if (!GetExitCodeProcess(process.process, &exit_code))
    return false;
  return exit_code == STILL_ACTIVE;
#else
  return kill(static_cast<pid_t>(process.pid), 0) == 0;
#endif
}

void cleanup_dummy_process(DummyProcess *process) {
  if (!process)
    return;

#ifdef _WIN32
  if (process->process) {
    DWORD exit_code = 0;
    if (GetExitCodeProcess(process->process, &exit_code) &&
        exit_code == STILL_ACTIVE) {
      TerminateProcess(process->process, 1);
      WaitForSingleObject(process->process, 5000);
    }
    CloseHandle(process->process);
    process->process = NULL;
  }
  if (process->thread) {
    CloseHandle(process->thread);
    process->thread = NULL;
  }
#else
  if (process->pid > 0) {
    kill(static_cast<pid_t>(process->pid), SIGKILL);
    waitpid(static_cast<pid_t>(process->pid), nullptr, 0);
  }
#endif

  process->pid = -1;
}

void write_text(const std::string &path, const std::string &content) {
  std::ofstream out(path, std::ios::binary);
  out << content;
}

ecnuvpn::vpn_engine::NativeSessionRecord native_ready_record() {
  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "utun-native";
  metadata.internal_ip4_address = "10.44.0.8";
  metadata.internal_ip4_netmask = "255.255.255.255";
  metadata.routes = {"59.78.176.0/20"};

  ecnuvpn::vpn_engine::NativeSessionRecord record;
  record.session.tunnel_configured(metadata);
  record.session.packet_loop_started();
  record.pid = current_process_id();
  record.server = "vpn.example.edu";
  record.route_count = 1;
  return record;
}

ecnuvpn::vpn_engine::NativeSessionRecord native_configuring_record() {
  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "utun-native";
  metadata.internal_ip4_address = "10.44.0.8";
  metadata.internal_ip4_netmask = "255.255.255.255";
  metadata.routes = {"59.78.176.0/20"};

  ecnuvpn::vpn_engine::NativeSessionRecord record;
  record.session.tunnel_configured(metadata);
  record.pid = current_process_id();
  record.server = "vpn.example.edu";
  record.route_count = 1;
  return record;
}

} // namespace

int main() {
  namespace fs = std::filesystem;

  ecnuvpn::g_runtime_test_config = ecnuvpn::Config{};
  ecnuvpn::g_runtime_test_config.vpn_engine = "legacy_openconnect";

  const fs::path temp_root = fs::temp_directory_path() /
                             ("ecnuvpn-runtime-test-" +
                              std::to_string(current_process_id()));

  fs::remove_all(temp_root);
  fs::create_directories(temp_root);

  ecnuvpn::platform::set_runtime_path_override(temp_root.string(),
                                            temp_root.string());

  const auto empty_snapshot = ecnuvpn::vpn::read_runtime_status_snapshot();

  ecnuvpn::platform::write_file(ecnuvpn::platform::get_pid_path(),
                             std::to_string(current_process_id()));
  ecnuvpn::platform::write_file(ecnuvpn::platform::get_supervisor_pid_path(),
                             std::to_string(current_process_id()));
  ecnuvpn::platform::write_file(ecnuvpn::platform::get_route_ready_path(),
                             "utun9\n10.0.0.8\n");

  const auto snapshot = ecnuvpn::vpn::read_runtime_status_snapshot();

  bool ok = true;
  ok = expect(!empty_snapshot.running,
              "snapshot should stay disconnected without managed pid files") && ok;
  ok = expect(snapshot.running, "snapshot should report a running session") && ok;
  ok = expect(snapshot.pid == current_process_id(), "snapshot should use the live PID file") && ok;
  ok = expect(snapshot.supervisor_pid == current_process_id(), "snapshot should use the live supervisor PID file") && ok;
  ok = expect(snapshot.network_ready, "snapshot should report ready routes when route-ready exists") && ok;
  ok = expect(snapshot.interface_name == "utun9", "snapshot should read the interface name") && ok;
  ok = expect(snapshot.internal_ip == "10.0.0.8", "snapshot should read the internal IP") && ok;

  DummyProcess process = spawn_dummy_process();
  ok = expect(process.pid > 0, "dummy process should start") && ok;

  if (process.pid > 0) {
    ecnuvpn::platform::write_file(ecnuvpn::platform::get_pid_path(),
                               std::to_string(process.pid));
    ecnuvpn::platform::write_file(ecnuvpn::platform::get_supervisor_pid_path(),
                               std::to_string(process.pid));
    ecnuvpn::platform::write_file(ecnuvpn::platform::get_route_ready_path(),
                               "utun11\n10.0.0.9\n");

    const auto live_dummy_snapshot =
        ecnuvpn::vpn::read_runtime_status_snapshot();
    ok = expect(live_dummy_snapshot.running,
                "snapshot should report a running dummy session") && ok;
    ok = expect(live_dummy_snapshot.pid == process.pid,
                "snapshot should use the live dummy pid file") && ok;
    ok = expect(live_dummy_snapshot.supervisor_pid == process.pid,
                "snapshot should use the live dummy supervisor pid file") && ok;
    ok = expect(live_dummy_snapshot.interface_name == "utun11",
                "snapshot should update the interface name for dummy sessions") && ok;
    ok = expect(live_dummy_snapshot.internal_ip == "10.0.0.9",
                "snapshot should update the internal IP for dummy sessions") && ok;

    cleanup_dummy_process(&process);

    const auto stale_dummy_snapshot =
        ecnuvpn::vpn::read_runtime_status_snapshot();
    ok = expect(!stale_dummy_snapshot.running,
                "snapshot should treat stale dummy pid files as disconnected") && ok;
    ok = expect(stale_dummy_snapshot.pid == -1,
                "snapshot should clear stale dummy pid values") && ok;
    ok = expect(stale_dummy_snapshot.supervisor_pid == -1,
                "snapshot should clear stale dummy supervisor pid values") && ok;
  }

  ecnuvpn::Config native_cfg;
  native_cfg.vpn_engine = "native";

  int openconnect_fallback_calls = 0;
  ecnuvpn::vpn::RuntimeStatusProbe native_probe;
  native_probe.is_process_alive = [](int pid) {
    return pid == current_process_id();
  };
  native_probe.find_openconnect_pid = [&openconnect_fallback_calls]() {
    ++openconnect_fallback_calls;
    return current_process_id();
  };
  native_probe.interfaces_output = []() { return std::string("native-ifaces"); };

  ecnuvpn::vpn_engine::clear_native_session_state(temp_root.string());
  ok = expect(ecnuvpn::vpn_engine::save_native_session_state(
                  temp_root.string(), native_ready_record()),
              "native ready state should be saved for runtime status") &&
       ok;

  const auto native_snapshot =
      ecnuvpn::vpn::read_runtime_status_snapshot(native_cfg, native_probe);
  ok = expect(native_snapshot.running,
              "native runtime snapshot should use persisted native running state") &&
       ok;
  ok = expect(native_snapshot.network_ready,
              "native runtime snapshot should use persisted native network readiness") &&
       ok;
  ok = expect(native_snapshot.pid == current_process_id(),
              "native runtime snapshot should use persisted native pid") &&
       ok;
  ok = expect(native_snapshot.interface_name == "utun-native",
              "native runtime snapshot should expose native interface metadata") &&
       ok;
  ok = expect(native_snapshot.internal_ip == "10.44.0.8",
              "native runtime snapshot should expose native internal IP metadata") &&
       ok;
  ok = expect(openconnect_fallback_calls == 0,
              "native runtime snapshot must not call OpenConnect pid fallback") &&
       ok;

  ecnuvpn::g_runtime_test_config.vpn_engine = "native";
  const auto native_no_arg_snapshot =
      ecnuvpn::vpn::read_runtime_status_snapshot();
  ok = expect(native_no_arg_snapshot.running,
              "no-arg runtime snapshot should use loaded native config") &&
       ok;
  ok = expect(native_no_arg_snapshot.interface_name == "utun-native",
              "no-arg runtime snapshot should expose native metadata") &&
       ok;

  ecnuvpn::vpn_engine::clear_native_session_state(temp_root.string());
  ok = expect(ecnuvpn::vpn_engine::save_native_session_state(
                  temp_root.string(), native_configuring_record()),
              "native configuring state should be saved for runtime status") &&
       ok;

  const auto native_configuring_snapshot =
      ecnuvpn::vpn::read_runtime_status_snapshot(native_cfg, native_probe);
  ok = expect(native_configuring_snapshot.running,
              "native runtime snapshot should treat live pre-packet-loop state as running") &&
       ok;
  ok = expect(!native_configuring_snapshot.network_ready,
              "native runtime snapshot must not treat pre-packet-loop state as network-ready") &&
       ok;
  ok = expect(openconnect_fallback_calls == 0,
              "native pre-packet-loop status must still avoid OpenConnect fallback") &&
       ok;

  ecnuvpn::vpn_engine::clear_native_session_state(temp_root.string());
  ok = expect(ecnuvpn::vpn_engine::save_native_session_state(
                  temp_root.string(), native_ready_record()),
              "native ready state should be saved for conservative probe check") &&
       ok;
  ecnuvpn::vpn::RuntimeStatusProbe missing_liveness_probe;
  missing_liveness_probe.find_openconnect_pid = [&openconnect_fallback_calls]() {
    ++openconnect_fallback_calls;
    return current_process_id();
  };
  const auto native_without_liveness =
      ecnuvpn::vpn::read_runtime_status_snapshot(native_cfg,
                                                 missing_liveness_probe);
  ok = expect(!native_without_liveness.running,
              "native runtime snapshot must not trust pid without liveness probe") &&
       ok;
  ok = expect(openconnect_fallback_calls == 0,
              "native missing-liveness status must not use OpenConnect fallback") &&
       ok;

  ecnuvpn::vpn_engine::clear_native_session_state(temp_root.string());
  write_text(ecnuvpn::platform::get_route_ready_path(), "utun-marker\n10.0.0.99\n");
  const auto native_marker_only =
      ecnuvpn::vpn::read_runtime_status_snapshot(native_cfg, native_probe);
  ok = expect(!native_marker_only.running,
              "native marker-only status should stay disconnected") &&
       ok;
  ok = expect(openconnect_fallback_calls == 0,
              "native marker-only status must still avoid OpenConnect fallback") &&
       ok;

  ecnuvpn::Config legacy_cfg;
  legacy_cfg.vpn_engine = "legacy_openconnect";
  int legacy_fallback_calls = 0;
  ecnuvpn::vpn::RuntimeStatusProbe legacy_probe;
  legacy_probe.is_process_alive = [](int) { return false; };
  legacy_probe.find_openconnect_pid = [&legacy_fallback_calls]() {
    ++legacy_fallback_calls;
    return current_process_id();
  };
  legacy_probe.interfaces_output = []() { return std::string("legacy-ifaces"); };
  std::filesystem::remove(ecnuvpn::platform::get_pid_path());
  std::filesystem::remove(ecnuvpn::platform::get_supervisor_pid_path());
  write_text(ecnuvpn::platform::get_route_ready_path(), "utun-legacy\n10.0.0.7\n");

  const auto legacy_snapshot =
      ecnuvpn::vpn::read_runtime_status_snapshot(legacy_cfg, legacy_probe);
  ok = expect(legacy_snapshot.running,
              "legacy runtime snapshot should keep OpenConnect pid fallback") &&
       ok;
  ok = expect(legacy_snapshot.network_ready,
              "legacy runtime snapshot should keep route-ready compatibility") &&
       ok;
  ok = expect(legacy_snapshot.pid == current_process_id(),
              "legacy runtime snapshot should use OpenConnect fallback pid") &&
       ok;
  ok = expect(legacy_snapshot.pid_from_openconnect_scan,
              "legacy runtime snapshot should mark OpenConnect fallback pid source") &&
       ok;
  ok = expect(legacy_fallback_calls == 1,
              "legacy runtime snapshot should call OpenConnect pid fallback") &&
       ok;

  ecnuvpn::platform::clear_runtime_path_override();
  cleanup_dummy_process(&process);
  fs::remove_all(temp_root);

  return ok ? 0 : 1;
}
