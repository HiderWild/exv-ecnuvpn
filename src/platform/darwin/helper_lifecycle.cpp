#include "platform/common/helper_lifecycle.hpp"

#include "helper/helper_ipc.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/helper_platform.hpp"
#include "cli/console.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_paths.hpp"
#include "utils/strings.hpp"

#include <cctype>
#include <cerrno>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <filesystem>
#include <iostream>
#include <sstream>
#include <string>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

namespace exv {
namespace platform {
namespace {

struct SemanticVersion {
  int major = -1;
  int minor = -1;
  int patch = -1;
};
// Begin inlined from platform/darwin/helper_lifecycle_version include-unit
bool prompt_confirm(const std::string &question, bool default_yes) {
  if (!isatty(STDIN_FILENO))
    return true;

  std::cout << "  " << question << (default_yes ? " [Y/n]: " : " [y/N]: ");
  std::string input;
  std::getline(std::cin, input);
  input = exv::utils::trim(input);
  if (input.empty())
    return default_yes;
  return input[0] == 'y' || input[0] == 'Y';
}

bool parse_semantic_version_token(const std::string &token,
                                  SemanticVersion *version) {
  if (!version || token.empty())
    return false;

  std::istringstream iss(token);
  std::string part;
  std::vector<int> parts;
  while (std::getline(iss, part, '.')) {
    if (part.empty())
      return false;
    for (char ch : part) {
      if (!std::isdigit(static_cast<unsigned char>(ch)))
        return false;
    }
    try {
      parts.push_back(std::stoi(part));
    } catch (...) {
      return false;
    }
  }

  if (parts.size() != 3)
    return false;

  version->major = parts[0];
  version->minor = parts[1];
  version->patch = parts[2];
  return true;
}

bool parse_semantic_version(const std::string &text, SemanticVersion *version) {
  if (!version)
    return false;

  std::string candidate;
  auto flush_candidate = [&]() -> bool {
    if (candidate.empty())
      return false;
    SemanticVersion parsed;
    bool ok = parse_semantic_version_token(candidate, &parsed);
    candidate.clear();
    if (ok) {
      *version = parsed;
      return true;
    }
    return false;
  };

  for (char ch : text) {
    unsigned char uch = static_cast<unsigned char>(ch);
    if (std::isdigit(uch) || ch == '.') {
      candidate.push_back(ch);
    } else if (flush_candidate()) {
      return true;
    }
  }

  return flush_candidate();
}

std::string format_semantic_version(const SemanticVersion &version) {
  return std::to_string(version.major) + "." +
         std::to_string(version.minor) + "." +
         std::to_string(version.patch);
}

int compare_semantic_versions(const SemanticVersion &lhs,
                              const SemanticVersion &rhs) {
  if (lhs.major != rhs.major)
    return lhs.major < rhs.major ? -1 : 1;
  if (lhs.minor != rhs.minor)
    return lhs.minor < rhs.minor ? -1 : 1;
  if (lhs.patch != rhs.patch)
    return lhs.patch < rhs.patch ? -1 : 1;
  return 0;
}

bool read_binary_version(const std::string &path, SemanticVersion *version) {
  if (!version || !platform::file_exists(path))
    return false;

  std::string output = exv::utils::trim(
      platform::run_command_output(platform::shell_quote(path) + " version 2>/dev/null"));
  if (output.empty())
    return false;
  return parse_semantic_version(output, version);
}
// End inlined from platform/darwin/helper_lifecycle_version include-unit
// Begin inlined from platform/darwin/helper_lifecycle_install include-unit
bool uninstall_existing_stable_exv() {
  const auto &platform_config = helper_platform_config();
  if (!platform::file_exists(platform_config.stable_install_path))
    return true;

  cli::print_info(
      "Running service uninstall using the existing stable exv before replacement...");
  if (platform::run_command(platform::shell_quote(platform_config.stable_install_path) +
                         " service uninstall") != 0) {
    cli::print_error(
        "Failed to uninstall the existing helper service before replacing /usr/local/bin/exv.");
    return false;
  }
  return true;
}

bool copy_file_contents(const std::string &source_path,
                        const std::string &target_path,
                        int *error_number = nullptr) {
  if (error_number)
    *error_number = 0;

  int src_fd = open(source_path.c_str(), O_RDONLY);
  if (src_fd < 0) {
    if (error_number)
      *error_number = errno;
    return false;
  }

  std::string temp_template = target_path + ".tmp.XXXXXX";
  std::vector<char> temp_path(temp_template.begin(), temp_template.end());
  temp_path.push_back('\0');

  int dst_fd = mkstemp(temp_path.data());
  if (dst_fd < 0) {
    int saved_errno = errno;
    close(src_fd);
    if (error_number)
      *error_number = saved_errno;
    return false;
  }

  bool ok = true;
  int saved_errno = 0;
  char buffer[16384];
  while (ok) {
    ssize_t read_size = read(src_fd, buffer, sizeof(buffer));
    if (read_size == 0)
      break;
    if (read_size < 0) {
      if (errno == EINTR)
        continue;
      saved_errno = errno;
      ok = false;
      break;
    }

    ssize_t total_written = 0;
    while (total_written < read_size) {
      ssize_t write_size =
          write(dst_fd, buffer + total_written, read_size - total_written);
      if (write_size < 0) {
        if (errno == EINTR)
          continue;
        saved_errno = errno;
        ok = false;
        break;
      }
      total_written += write_size;
    }
  }

  if (ok && fsync(dst_fd) != 0) {
    saved_errno = errno;
    ok = false;
  }
  if (ok && chmod(temp_path.data(), 0755) != 0) {
    saved_errno = errno;
    ok = false;
  }

  close(src_fd);
  if (close(dst_fd) != 0 && ok) {
    saved_errno = errno;
    ok = false;
  }

  if (ok && rename(temp_path.data(), target_path.c_str()) != 0) {
    saved_errno = errno;
    ok = false;
  }

  if (!ok) {
    std::remove(temp_path.data());
    if (error_number)
      *error_number = saved_errno;
    errno = saved_errno;
    return false;
  }

  return true;
}

std::string helper_binary_next_to(const std::string &executable_path) {
  std::filesystem::path path(executable_path);
  return (path.parent_path() / "exv-helper").string();
}

} // namespace

std::string get_interfaces_output() {
  return platform::run_command_output("ifconfig | grep -A 2 'utun' | head -20");
}

void fix_config_dir_ownership() {
  platform::fix_runtime_config_dir_ownership();
}

int copy_self_to_stable_path_and_reexec(const std::string &current_path) {
  const auto &platform_config = helper_platform_config();
  SemanticVersion current_version;
  bool current_version_ok =
      parse_semantic_version(EXV_VERSION, &current_version);
  bool stable_exists = platform::file_exists(platform_config.stable_install_path);

  cli::print_warning(
      "EXV helper service should be installed from a stable system path.");
  std::cout << cli::DIM << "  Current executable: " << current_path
            << cli::RESET << std::endl;
  std::cout << cli::DIM << "  Stable target: "
            << platform_config.stable_install_path << cli::RESET << std::endl;
  if (current_version_ok) {
    std::cout << cli::DIM << "  Current version: "
              << format_semantic_version(current_version) << cli::RESET
              << std::endl;
  }

  bool proceed = false;
  if (!stable_exists) {
    std::cout << std::endl;
    proceed = prompt_confirm(
        "No exv binary was found at the stable target. Copy this binary there and re-run service installation from that location?",
        true);
  } else {
    SemanticVersion stable_version;
    bool stable_version_ok =
        read_binary_version(platform_config.stable_install_path, &stable_version);
    if (stable_version_ok) {
      std::cout << cli::DIM << "  Existing stable version: "
                << format_semantic_version(stable_version) << cli::RESET
                << std::endl;
    } else {
      std::cout << cli::DIM << "  Existing stable version: unknown"
                << cli::RESET << std::endl;
    }

    std::cout << std::endl;
    if (stable_version_ok && current_version_ok) {
      int cmp = compare_semantic_versions(stable_version, current_version);
      if (cmp == 0) {
        proceed = prompt_confirm(
            "The stable exv already matches this version. Reinstall it and refresh the helper service?",
            false);
      } else if (cmp < 0) {
        proceed = prompt_confirm(
            "The stable exv is older than this build. Upgrade it and reinstall the helper service?",
            true);
      } else {
        proceed = prompt_confirm(
            "The stable exv is newer than this build. Downgrade it and reinstall the helper service?",
            false);
      }
    } else {
      proceed = prompt_confirm(
          "An existing exv was found at the stable target, but its version could not be compared reliably. Replace it and reinstall the helper service?",
          false);
    }
  }

  if (!proceed) {
    cli::print_info("Service installation canceled.");
    return 1;
  }

  if (stable_exists && !uninstall_existing_stable_exv()) {
    return 1;
  }

  if (!platform::ensure_dir("/usr/local") || !platform::ensure_dir("/usr/local/bin")) {
    cli::print_error("Failed to ensure /usr/local/bin exists.");
    return 1;
  }

  cli::print_info("Copying current exv binary to /usr/local/bin/exv ...");
  int copy_error = 0;
  if (!copy_file_contents(current_path, platform_config.stable_install_path,
                          &copy_error)) {
    cli::print_error("Failed to copy exv to /usr/local/bin/exv: " +
                       std::string(std::strerror(copy_error)));
    return 1;
  }

  cli::print_success("Stable exv binary updated at /usr/local/bin/exv.");
  std::string helper_source = helper_binary_next_to(current_path);
  if (!platform::file_exists(helper_source)) {
    cli::print_error("Expected exv-helper next to the current exv binary: " +
                       helper_source);
    return 1;
  }

  std::filesystem::path stable_helper_path(
      platform_config.default_service_binary_path);
  std::error_code helper_dir_error;
  std::filesystem::create_directories(stable_helper_path.parent_path(),
                                      helper_dir_error);
  if (helper_dir_error) {
    cli::print_error("Failed to create stable helper directory: " +
                     helper_dir_error.message());
    return 1;
  }

  cli::print_info("Copying exv-helper binary to " +
                  stable_helper_path.string() + " ...");
  if (!copy_file_contents(helper_source,
                          platform_config.default_service_binary_path,
                          &copy_error)) {
    cli::print_error("Failed to copy exv-helper to " +
                       stable_helper_path.string() + ": " +
                       std::string(std::strerror(copy_error)));
    return 1;
  }
  cli::print_success("Stable exv-helper binary updated at " +
                     stable_helper_path.string() + ".");

  cli::print_info("Re-running service installation from the copied binary...");
  execl(platform_config.stable_install_path, platform_config.stable_install_path,
        "service", "install", static_cast<char *>(nullptr));

  cli::print_error("Failed to launch /usr/local/bin/exv: " +
                     std::string(std::strerror(errno)));
  return 1;
}
// End inlined from platform/darwin/helper_lifecycle_install include-unit
// Begin inlined from platform/darwin/helper_lifecycle_worker include-unit
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

  // Wait for the child to finish writing the response before closing
  // the client fd.  The child inherited client_fd_ across fork(); if
  // the parent closes it first, the child's write() fails and the
  // client receives an empty response.
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
// End inlined from platform/darwin/helper_lifecycle_worker include-unit
} // namespace platform
} // namespace exv
