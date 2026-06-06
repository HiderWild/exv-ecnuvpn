#include "vpn.hpp"

#include "platform/common/process_control.hpp"
#include "utils.hpp"
#include "vpn_engine/native_session_store.hpp"

#include <cerrno>
#include <fstream>
#include <sstream>

namespace ecnuvpn {
namespace vpn {
namespace {

static pid_t read_pid_file(const std::string &path) {
  if (!utils::file_exists(path))
    return -1;

  std::string content = utils::trim(utils::read_file(path));
  if (content.empty())
    return -1;

  try {
    return static_cast<pid_t>(std::stoi(content));
  } catch (...) {
    return -1;
  }
}

static bool is_process_alive(pid_t pid) {
  return platform::is_process_alive(static_cast<int>(pid));
}

static std::string read_interfaces_output() {
#ifdef __APPLE__
  return utils::run_command_output("ifconfig | grep -A 2 'utun' | head -20");
#elif defined(_WIN32)
  return utils::run_command_output("netsh interface show interface 2>nul");
#else
  return utils::run_command_output("ip addr show type tun 2>/dev/null | head -20");
#endif
}

RuntimeStatusProbe default_probe() {
  RuntimeStatusProbe probe;
  probe.is_process_alive = [](int pid) {
    return platform::is_process_alive(pid);
  };
  probe.find_openconnect_pid = []() { return platform::find_openconnect_pid(); };
  probe.interfaces_output = []() { return read_interfaces_output(); };
  return probe;
}

bool probe_process_alive(const RuntimeStatusProbe &probe, pid_t pid) {
  if (pid <= 0)
    return false;
  if (!probe.is_process_alive)
    return is_process_alive(pid);
  return probe.is_process_alive(static_cast<int>(pid));
}

int probe_find_openconnect_pid(const RuntimeStatusProbe &probe) {
  if (probe.find_openconnect_pid)
    return probe.find_openconnect_pid();
  return platform::find_openconnect_pid();
}

std::string probe_interfaces_output(const RuntimeStatusProbe &probe) {
  if (probe.interfaces_output)
    return probe.interfaces_output();
  return read_interfaces_output();
}

static bool read_route_ready(std::string *interface_name,
                             std::string *internal_ip) {
  std::string path = utils::get_route_ready_path();
  if (!utils::file_exists(path))
    return false;

  std::istringstream iss(utils::read_file(path));
  std::string tun;
  std::string ip;
  if (!std::getline(iss, tun) || !std::getline(iss, ip))
    return false;

  tun = utils::trim(tun);
  ip = utils::trim(ip);
  if (tun.empty() || ip.empty())
    return false;

  if (interface_name)
    *interface_name = tun;
  if (internal_ip)
    *internal_ip = ip;
  return true;
}

} // namespace

RuntimeStatusSnapshot read_runtime_status_snapshot() {
  return read_runtime_status_snapshot(config::load());
}

RuntimeStatusSnapshot read_runtime_status_snapshot(const Config &cfg) {
  return read_runtime_status_snapshot(cfg, default_probe());
}

RuntimeStatusSnapshot read_runtime_status_snapshot(const Config &cfg,
                                                   const RuntimeStatusProbe &probe) {
  RuntimeStatusSnapshot snapshot;

  if (cfg.vpn_engine == "native") {
    // D3: This path reads native-session-state.json for status when the
    // legacy supervisor fallback is active.  The TunnelController path
    // (app_api.cpp status.get) reads from in-memory TunnelStatusSnapshot
    // instead.  This code path is used by: CLI 'exv status', direct fallback
    // connect (linux/darwin), and status_fallback_without_helper.
    vpn_engine::NativeSessionProbe native_probe;
    native_probe.is_process_alive = probe.is_process_alive;
    vpn_engine::NativeSessionSnapshot native =
        vpn_engine::read_native_session_snapshot(utils::get_config_dir(),
                                                 native_probe);

    snapshot.running = native.running;
    snapshot.pid = native.pid;
    snapshot.supervisor_pid = native.supervisor_pid;
    snapshot.network_ready = native.network_ready;
    snapshot.interface_name = native.interface_name;
    snapshot.internal_ip = native.internal_ip;
    if (snapshot.running)
      snapshot.interfaces_output = probe_interfaces_output(probe);
    return snapshot;
  }

  pid_t supervisor_pid = read_pid_file(utils::get_supervisor_pid_path());
  if (supervisor_pid <= 0 || !probe_process_alive(probe, supervisor_pid))
    supervisor_pid = -1;

  pid_t pid = read_pid_file(utils::get_pid_path());
  bool pid_from_openconnect_scan = false;
  if (pid <= 0 || !probe_process_alive(probe, pid)) {
    pid = static_cast<pid_t>(probe_find_openconnect_pid(probe));
    pid_from_openconnect_scan = pid > 0;
  }

  snapshot.pid = pid > 0 ? static_cast<int>(pid) : -1;
  snapshot.pid_from_openconnect_scan = pid_from_openconnect_scan;
  snapshot.supervisor_pid = supervisor_pid > 0 ? static_cast<int>(supervisor_pid)
                                               : -1;
  snapshot.running = snapshot.pid > 0 || snapshot.supervisor_pid > 0;
  snapshot.network_ready =
      read_route_ready(&snapshot.interface_name, &snapshot.internal_ip);

  if (snapshot.running)
    snapshot.interfaces_output = probe_interfaces_output(probe);

  return snapshot;
}

} // namespace vpn
} // namespace ecnuvpn
