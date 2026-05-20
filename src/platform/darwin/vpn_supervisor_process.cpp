#include "platform/common/vpn_supervisor_process.hpp"

#include <unistd.h>

namespace ecnuvpn {
namespace platform {

bool spawn_vpn_supervisor_process(const Config &cfg,
                                  const std::string &password,
                                  int retry_limit,
                                  SupervisorEntryPoint entry_point,
                                  int *pid) {
  if (pid)
    *pid = -1;

  pid_t child_pid = fork();
  if (child_pid < 0) {
    return false;
  }

  if (child_pid == 0) {
    if (setsid() < 0) {
      _exit(1);
    }
    int result = entry_point ? entry_point(cfg, password, retry_limit) : 1;
    _exit(result == 0 ? 0 : 1);
  }

  if (pid)
    *pid = static_cast<int>(child_pid);
  return true;
}

} // namespace platform
} // namespace ecnuvpn