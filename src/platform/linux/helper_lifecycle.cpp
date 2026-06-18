#include "utils/strings.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/helper_lifecycle.hpp"

#include "helper/helper_ipc.hpp"
#include "observability/log_facade.hpp"

#include <cerrno>
#include <csignal>
#include <sstream>
#include <string>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

namespace ecnuvpn {
namespace platform {
namespace {

} // namespace

std::string get_interfaces_output() {
  return platform::run_command_output("ip addr show type tun 2>/dev/null | head -20");
}

void fix_config_dir_ownership() {
  platform::fix_runtime_config_dir_ownership();
}

int copy_self_to_stable_path_and_reexec(const std::string &) {
  return 1;
}

std::string create_temp_request_file(const std::string &payload) {
  char path_template[] = "/var/run/exv-helper-request-XXXXXX";
  int fd = mkstemp(path_template);
  if (fd < 0)
    return "";

  chmod(path_template, 0600);
  ssize_t written = write(fd, payload.data(), payload.size());
  close(fd);
  if (written != static_cast<ssize_t>(payload.size())) {
    std::remove(path_template);
    return "";
  }

  return path_template;
}

int spawn_worker_process(const std::string &executable_path,
                         const std::string &request_path) {
  (void)executable_path;
  (void)request_path;
  return 1;
}

void terminate_process(int pid) {
  if (pid <= 0)
    return;
  kill(pid, SIGTERM);
}

void force_terminate_process(int pid) {
  if (pid <= 0)
    return;
  kill(pid, SIGKILL);
}

void sleep_ms(int milliseconds) {
  usleep(static_cast<useconds_t>(milliseconds) * 1000);
}

void reap_children() {
  int status = 0;
  while (waitpid(-1, &status, WNOHANG) > 0) {
  }
}

void dispatch_request_background(
    helper::IpcServer &ipc, const std::string &raw_request,
    unsigned int peer_uid, unsigned int peer_gid,
    std::function<nlohmann::json(unsigned int, unsigned int,
                                  const nlohmann::json &)> handler) {
  pid_t handler_pid = fork();
  if (handler_pid < 0) {
    nlohmann::json response =
        nlohmann::json{{"ok", false}, {"message", "Failed to launch EXV helper request handler."}};
    ipc.send_response(response.dump());
    ipc.close_client();
    return;
  }

  if (handler_pid == 0) {
    signal(SIGTERM, SIG_DFL);
    signal(SIGINT, SIG_DFL);
    signal(SIGCHLD, SIG_DFL);
    signal(SIGPIPE, SIG_IGN);
    ipc.close_server();
    nlohmann::json response;
    try {
      nlohmann::json request = nlohmann::json::parse(raw_request);
      response = handler(peer_uid, peer_gid, request);
    } catch (...) {
      response = nlohmann::json{{"ok", false}, {"message", "Failed to parse helper request."}};
    }
    ipc.send_response(response.dump());
    _exit(0);
  }

  int status = 0;
  while (waitpid(handler_pid, &status, 0) < 0 && errno == EINTR)
    ;
  ipc.close_client();
}

void set_session_state_permissions(const std::string &path) {
  chmod(path.c_str(), 0600);
}

void setup_daemon_signals() {
  signal(SIGPIPE, SIG_IGN);
}

void cleanup_daemon_endpoint(const std::string &endpoint) {
  std::remove(endpoint.c_str());
}

} // namespace platform
} // namespace ecnuvpn
