#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/process_control.hpp"


#include <string>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

namespace ecnuvpn {
namespace platform {

bool is_process_alive(int pid) {
  if (pid <= 0)
    return false;

  HANDLE h_process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE,
                                 static_cast<DWORD>(pid));
  if (!h_process)
    return false;

  DWORD exit_code = 0;
  BOOL ok = GetExitCodeProcess(h_process, &exit_code);
  CloseHandle(h_process);
  return ok && exit_code == STILL_ACTIVE;
}

int find_openconnect_pid() {
  std::string output = platform::run_command_output(
      "tasklist /FI \"IMAGENAME eq openconnect.exe\" /NH /FO CSV 2>nul");
  auto start = output.find('"', output.find(',') + 1);
  if (start == std::string::npos)
    return -1;
  auto end = output.find('"', start + 1);
  if (end == std::string::npos)
    return -1;
  std::string pid_str = output.substr(start + 1, end - start - 1);
  try {
    return std::stoi(pid_str);
  } catch (...) {
    return -1;
  }
}

bool terminate_process(int pid, bool force) {
  (void)force;
  if (pid <= 0)
    return false;

  HANDLE h_process = OpenProcess(PROCESS_TERMINATE, FALSE,
                                 static_cast<DWORD>(pid));
  if (!h_process)
    return false;

  BOOL ok = TerminateProcess(h_process, 1);
  CloseHandle(h_process);
  return ok == TRUE;
}

void sleep_ms(unsigned int milliseconds) {
  Sleep(milliseconds);
}

} // namespace platform
} // namespace ecnuvpn
