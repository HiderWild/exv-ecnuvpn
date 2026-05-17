#include "utils.hpp"
#include "vpn.hpp"

#include <cstdlib>
#include <filesystem>
#include <iostream>

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
  char command_line[] = "cmd.exe /C ping -n 60 127.0.0.1 > nul";

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

  ecnuvpn::utils::write_file(ecnuvpn::utils::get_pid_path(),
                             std::to_string(current_process_id()));
  ecnuvpn::utils::write_file(ecnuvpn::utils::get_supervisor_pid_path(),
                             std::to_string(current_process_id()));
  ecnuvpn::utils::write_file(ecnuvpn::utils::get_route_ready_path(),
                             "utun9\n10.0.0.8\n");

  const auto snapshot = ecnuvpn::vpn::read_runtime_status_snapshot();

  bool ok = true;
  ok = expect(snapshot.running, "snapshot should report a running session") && ok;
  ok = expect(snapshot.pid == current_process_id(), "snapshot should use the live PID file") && ok;
  ok = expect(snapshot.supervisor_pid == current_process_id(), "snapshot should use the live supervisor PID file") && ok;
  ok = expect(snapshot.network_ready, "snapshot should report ready routes when route-ready exists") && ok;
  ok = expect(snapshot.interface_name == "utun9", "snapshot should read the interface name") && ok;
  ok = expect(snapshot.internal_ip == "10.0.0.8", "snapshot should read the internal IP") && ok;

  DummyProcess process = spawn_dummy_process();
  ok = expect(process.pid > 0, "dummy process should start") && ok;

  if (process.pid > 0) {
    ecnuvpn::utils::write_file(ecnuvpn::utils::get_pid_path(),
                               std::to_string(process.pid));
    ecnuvpn::utils::write_file(ecnuvpn::utils::get_route_ready_path(),
                               "utun11\n10.0.0.9\n");

    ok = expect(ecnuvpn::vpn::stop_direct_session(),
                "direct stop should succeed for a live pid file") && ok;
    ok = expect(!is_dummy_process_alive(process),
                "direct stop should terminate the dummy process") && ok;
    ok = expect(!ecnuvpn::utils::file_exists(ecnuvpn::utils::get_pid_path()),
                "direct stop should remove the pid file") && ok;
    ok = expect(!ecnuvpn::utils::file_exists(ecnuvpn::utils::get_route_ready_path()),
                "direct stop should remove the route-ready file") && ok;
  }

  ecnuvpn::utils::clear_runtime_path_override();
  cleanup_dummy_process(&process);
  fs::remove_all(temp_root);

  return ok ? 0 : 1;
}