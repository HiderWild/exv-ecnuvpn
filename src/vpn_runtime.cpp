#include "vpn.hpp"

#include "utils.hpp"

#include <cerrno>
#include <fstream>
#include <sstream>

#ifndef _WIN32
#include <csignal>
#include <unistd.h>
#else
#include <windows.h>
#endif

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

static void remove_runtime_file(const std::string &path) {
  if (utils::file_exists(path)) {
    std::remove(path.c_str());
  }
}

static void clear_runtime_state() {
  remove_runtime_file(utils::get_pid_path());
  remove_runtime_file(utils::get_supervisor_pid_path());
  remove_runtime_file(utils::get_route_ready_path());
}

static bool is_process_alive(pid_t pid) {
  if (pid <= 0)
    return false;

#ifndef _WIN32
  if (kill(pid, 0) == 0)
    return true;
  return errno == EPERM;
#else
  HANDLE h_process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE,
                                 static_cast<DWORD>(pid));
  if (!h_process)
    return false;

  DWORD exit_code = 0;
  BOOL ok = GetExitCodeProcess(h_process, &exit_code);
  CloseHandle(h_process);
  return ok && exit_code == STILL_ACTIVE;
#endif
}

static void terminate_process(pid_t pid, bool force) {
  if (pid <= 0)
    return;

#ifndef _WIN32
  kill(pid, force ? SIGKILL : SIGTERM);
#else
  HANDLE h_process = OpenProcess(PROCESS_TERMINATE, FALSE,
                                 static_cast<DWORD>(pid));
  if (!h_process)
    return;

  TerminateProcess(h_process, force ? 1 : 0);
  CloseHandle(h_process);
#endif
}

static void wait_briefly() {
#ifndef _WIN32
  usleep(300000);
#else
  Sleep(300);
#endif
}

static pid_t find_openconnect_pid() {
#ifndef _WIN32
  std::string output = utils::trim(utils::run_command_output("pgrep -x openconnect"));
  if (output.empty())
    return -1;

  try {
    return static_cast<pid_t>(std::stoi(output));
  } catch (...) {
    return -1;
  }
#else
  std::string output = utils::run_command_output(
      "tasklist /FI \"IMAGENAME eq openconnect.exe\" /NH /FO CSV 2>nul");
  auto start = output.find('"', output.find(',') + 1);
  if (start == std::string::npos)
    return -1;

  auto end = output.find('"', start + 1);
  if (end == std::string::npos)
    return -1;

  try {
    return static_cast<pid_t>(std::stoi(output.substr(start + 1, end - start - 1)));
  } catch (...) {
    return -1;
  }
#endif
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

static std::string read_interfaces_output() {
#ifdef __APPLE__
  return utils::run_command_output("ifconfig | grep -A 2 'utun' | head -20");
#elif defined(_WIN32)
  return utils::run_command_output("netsh interface show interface 2>nul");
#else
  return utils::run_command_output("ip addr show type tun 2>/dev/null | head -20");
#endif
}

} // namespace

RuntimeStatusSnapshot read_runtime_status_snapshot() {
  RuntimeStatusSnapshot snapshot;

  pid_t supervisor_pid = read_pid_file(utils::get_supervisor_pid_path());
  if (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))
    supervisor_pid = -1;

  pid_t pid = read_pid_file(utils::get_pid_path());
  if (pid <= 0 || !is_process_alive(pid))
    pid = find_openconnect_pid();

  snapshot.pid = pid > 0 ? static_cast<int>(pid) : -1;
  snapshot.supervisor_pid = supervisor_pid > 0 ? static_cast<int>(supervisor_pid)
                                               : -1;
  snapshot.running = snapshot.pid > 0 || snapshot.supervisor_pid > 0;
  snapshot.network_ready =
      read_route_ready(&snapshot.interface_name, &snapshot.internal_ip);

  if (snapshot.running)
    snapshot.interfaces_output = read_interfaces_output();

  return snapshot;
}

bool stop_direct_session() {
  RuntimeStatusSnapshot snapshot = read_runtime_status_snapshot();
  if (!snapshot.running) {
    clear_runtime_state();
    return true;
  }

  pid_t supervisor_pid = static_cast<pid_t>(snapshot.supervisor_pid);
  pid_t pid = static_cast<pid_t>(snapshot.pid);

  if (supervisor_pid > 0)
    terminate_process(supervisor_pid, false);
  if (pid > 0)
    terminate_process(pid, false);

  for (int i = 0; i < 10; ++i) {
    if ((pid <= 0 || !is_process_alive(pid)) &&
        (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))) {
      break;
    }
    wait_briefly();
  }

  if (pid > 0 && is_process_alive(pid))
    terminate_process(pid, true);
  if (supervisor_pid > 0 && is_process_alive(supervisor_pid))
    terminate_process(supervisor_pid, true);

  wait_briefly();

  pid_t remaining_pid = find_openconnect_pid();
  if (remaining_pid > 0 && remaining_pid != pid)
    terminate_process(remaining_pid, true);

  wait_briefly();

  if ((pid > 0 && is_process_alive(pid)) ||
      (supervisor_pid > 0 && is_process_alive(supervisor_pid)) ||
      (remaining_pid > 0 && is_process_alive(remaining_pid))) {
    return false;
  }

  clear_runtime_state();
  return true;
}

} // namespace vpn
} // namespace ecnuvpn