#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "core/vpn/vpn.hpp"

#include "platform/common/process_control.hpp"

#include <cerrno>
#include <fstream>
#include <sstream>

namespace ecnuvpn {
namespace vpn {
namespace {

static std::string read_interfaces_output() {
#ifdef __APPLE__
  return platform::run_command_output("ifconfig | grep -A 2 'utun' | head -20");
#elif defined(_WIN32)
  return platform::run_command_output("netsh interface show interface 2>nul");
#else
  return platform::run_command_output("ip addr show type tun 2>/dev/null | head -20");
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

  // Status is now owned by the Core Process (TunnelController).
  // When the core is not running, report disconnected.
  snapshot.running = false;
  snapshot.pid = -1;
  snapshot.network_ready = false;
  snapshot.interfaces_output = probe.interfaces_output();

  return snapshot;
}

} // namespace vpn
} // namespace ecnuvpn
