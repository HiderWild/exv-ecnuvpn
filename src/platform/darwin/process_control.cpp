#include "utils/strings.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/process_control.hpp"


#include <cerrno>
#include <csignal>
#include <string>
#include <unistd.h>

namespace ecnuvpn {
namespace platform {

bool is_process_alive(int pid) {
  if (pid <= 0)
    return false;
  if (kill(static_cast<pid_t>(pid), 0) == 0)
    return true;
  return errno == EPERM;
}

int find_openconnect_pid() {
  std::string output = platform::run_command_output("pgrep -x openconnect");
  output = exv::utils::trim(output);
  if (output.empty())
    return -1;
  try {
    return std::stoi(output);
  } catch (...) {
    return -1;
  }
}

bool terminate_process(int pid, bool force) {
  if (pid <= 0)
    return false;
  return kill(static_cast<pid_t>(pid), force ? SIGKILL : SIGTERM) == 0;
}

void sleep_ms(unsigned int milliseconds) {
  usleep(milliseconds * 1000);
}

} // namespace platform
} // namespace ecnuvpn
