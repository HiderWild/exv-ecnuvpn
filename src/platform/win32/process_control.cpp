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
