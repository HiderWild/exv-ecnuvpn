#include "utils.hpp"
#include "vpn.hpp"

#include <filesystem>
#include <iostream>

#ifndef _WIN32
#include <unistd.h>
#endif

namespace {

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

} // namespace

int main() {
  namespace fs = std::filesystem;

  const fs::path temp_root = fs::temp_directory_path() /
                             ("ecnuvpn-runtime-test-" +
                              std::to_string(current_process_id()));

  fs::remove_all(temp_root);
  fs::create_directories(temp_root);

  ecnuvpn::utils::set_runtime_path_override(temp_root.string(),
                                            temp_root.string());

  const auto empty_snapshot = ecnuvpn::vpn::read_runtime_status_snapshot();

  ecnuvpn::utils::write_file(ecnuvpn::utils::get_pid_path(),
                             std::to_string(current_process_id()));
  ecnuvpn::utils::write_file(ecnuvpn::utils::get_supervisor_pid_path(),
                             std::to_string(current_process_id()));
  ecnuvpn::utils::write_file(ecnuvpn::utils::get_route_ready_path(),
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

  ecnuvpn::utils::clear_runtime_path_override();
  fs::remove_all(temp_root);

  return ok ? 0 : 1;
}