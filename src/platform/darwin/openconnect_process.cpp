#include "platform/common/openconnect_process.hpp"

#include "utils.hpp"

#include <sstream>
#include <unistd.h>

namespace ecnuvpn {
namespace platform {
namespace {

std::string build_openconnect_command(const Config &cfg,
                                      const std::string &password) {
  std::ostringstream cmd;
  std::string openconnect_path = utils::get_openconnect_path(cfg.openconnect_runtime);
  std::string heredoc_marker = "__EXV_PASSWORD_EOF__";
  while (password.find(heredoc_marker) != std::string::npos) {
    heredoc_marker += "_X";
  }

  cmd << "exec "
      << utils::shell_quote(openconnect_path.empty() ? std::string("openconnect")
                                                     : openconnect_path)
      << " " << utils::shell_quote(cfg.server)
      << " --useragent " << utils::shell_quote(cfg.useragent)
      << " -m " << cfg.mtu << " -u " << utils::shell_quote(cfg.username)
      << " --passwd-on-stdin"
      << " --non-inter"
      << " --script " << utils::shell_quote(utils::get_tunnel_path());

  if (cfg.disable_dtls) {
    cmd << " --no-dtls";
  }

  for (const auto &arg : cfg.extra_args) {
    cmd << " " << utils::shell_quote(arg);
  }

  cmd << " <<'" << heredoc_marker << "' >> "
      << utils::shell_quote(utils::expand_home(cfg.log_file)) << " 2>&1\n"
      << password << "\n" << heredoc_marker;
  return cmd.str();
}

} // namespace

bool spawn_openconnect_process(const Config &cfg, const std::string &password,
                               OpenconnectProcess *process) {
  if (process) {
    process->pid = -1;
    process->wait_handle = nullptr;
  }

  pid_t child_pid = fork();
  if (child_pid < 0) {
    return false;
  }

  if (child_pid == 0) {
    std::string cmd = build_openconnect_command(cfg, password);
    execl("/bin/sh", "sh", "-c", cmd.c_str(), static_cast<char *>(nullptr));
    _exit(127);
  }

  if (process) {
    process->pid = static_cast<int>(child_pid);
  }
  return true;
}

void close_openconnect_process(OpenconnectProcess *process) {
  (void)process;
}

} // namespace platform
} // namespace ecnuvpn
