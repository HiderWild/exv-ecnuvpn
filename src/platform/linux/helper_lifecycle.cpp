#include "platform/common/helper_lifecycle.hpp"

#include "logger.hpp"
#include "tunnel.hpp"
#include "utils.hpp"

#include <cerrno>
#include <csignal>
#include <sstream>
#include <string>
#include <unistd.h>

namespace ecnuvpn {
namespace platform {
namespace {

bool is_process_alive(pid_t pid) {
  if (pid <= 0)
    return false;
  if (kill(pid, 0) == 0)
    return true;
  return errno == EPERM;
}

} // namespace

void cleanup_routes() {
  tunnel::cleanup_routes();
}

void kill_all_supervisors() {
  std::string output = utils::trim(utils::run_command_output("pgrep -f 'exv -rt'"));
  if (output.empty())
    return;

  std::istringstream iss(output);
  std::string line;
  while (std::getline(iss, line)) {
    line = utils::trim(line);
    if (line.empty())
      continue;
    try {
      pid_t pid = static_cast<pid_t>(std::stoi(line));
      if (pid > 0 && is_process_alive(pid)) {
        logger::info("Killing orphaned supervisor: PID " + line);
        kill(pid, SIGKILL);
      }
    } catch (...) {
    }
  }
  usleep(500000);
}

void fix_config_dir_ownership() {
  utils::fix_config_dir_ownership();
}

int copy_self_to_stable_path_and_reexec(const std::string &) {
  return 1;
}

} // namespace platform
} // namespace ecnuvpn
