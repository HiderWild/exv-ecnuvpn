#include "helper/platform/helper_lifecycle.hpp"

#include "helper/helper_ipc.hpp"
#include "logger.hpp"
#include "tunnel.hpp"
#include "utils.hpp"

#include <cerrno>
#include <csignal>
#include <sstream>
#include <string>
#include <sys/wait.h>
#include <unistd.h>

namespace ecnuvpn {
namespace platform {
namespace {

} // namespace

void cleanup_routes() {
  tunnel::cleanup_routes();
}

int find_openconnect_pid() {
  std::string output = utils::trim(utils::run_command_output("pgrep -x openconnect"));
  if (output.empty())
    return -1;
  std::istringstream iss(output);
  std::string first;
  std::getline(iss, first);
  try {
    return std::stoi(first);
  } catch (...) {
    return -1;
  }
}

std::string get_interfaces_output() {
  return utils::run_command_output("ip addr show type tun 2>/dev/null | head -20");
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
  pid_t worker_pid = fork();
  if (worker_pid < 0)
    return -1;

  if (worker_pid == 0) {
    int devnull = open("/dev/null", O_WRONLY);
    if (devnull >= 0) {
      dup2(devnull, STDOUT_FILENO);
      dup2(devnull, STDERR_FILENO);
      close(devnull);
    }
    execl(executable_path.c_str(), executable_path.c_str(), "__helper-exec",
          request_path.c_str(), static_cast<char *>(nullptr));
    _exit(127);
  }

  int status = 0;
  while (waitpid(worker_pid, &status, 0) < 0) {
    if (errno != EINTR) {
      status = -1;
      break;
    }
  }
  if (status < 0)
    return -1;
  if (WIFEXITED(status))
    return WEXITSTATUS(status);
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
