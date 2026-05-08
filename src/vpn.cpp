#include "vpn.hpp"
#include "config.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "tunnel.hpp"
#include "utils.hpp"

#include <cerrno>
#include <csignal>
#include <fstream>
#include <iostream>
#include <sstream>
#include <sys/wait.h>
#include <unistd.h>

namespace ecnuvpn {
namespace vpn {

static volatile sig_atomic_t supervisor_stop_requested = 0;
static volatile sig_atomic_t supervisor_child_pid = -1;

static void write_pid_file(const std::string &path, pid_t pid) {
  std::ofstream ofs(path);
  if (ofs.is_open()) {
    ofs << pid;
    ofs.flush();
    utils::sync_owner(path);
  }
}

static pid_t read_pid_file(const std::string &path) {
  if (!utils::file_exists(path))
    return -1;
  std::string content = utils::read_file(path);
  content = utils::trim(content);
  if (content.empty())
    return -1;
  try {
    return static_cast<pid_t>(std::stoi(content));
  } catch (...) {
    return -1;
  }
}

static void remove_pid_file(const std::string &path) {
  if (utils::file_exists(path)) {
    std::remove(path.c_str());
  }
}

static void write_pid(pid_t pid) { write_pid_file(utils::get_pid_path(), pid); }

static pid_t read_pid() { return read_pid_file(utils::get_pid_path()); }

static void remove_pid() { remove_pid_file(utils::get_pid_path()); }

static void write_supervisor_pid(pid_t pid) {
  write_pid_file(utils::get_supervisor_pid_path(), pid);
}

static pid_t read_supervisor_pid() {
  return read_pid_file(utils::get_supervisor_pid_path());
}

static void remove_supervisor_pid() {
  remove_pid_file(utils::get_supervisor_pid_path());
}

static void remove_route_ready() {
  remove_pid_file(utils::get_route_ready_path());
}

static bool read_route_ready(std::string *interface_name = nullptr,
                             std::string *internal_ip = nullptr) {
  std::string path = utils::get_route_ready_path();
  if (!utils::file_exists(path))
    return false;

  std::istringstream iss(utils::read_file(path));
  std::string tun;
  std::string ip;
  if (!std::getline(iss, tun) || !std::getline(iss, ip))
    return false;

  tun = utils::trim(tun);
  ip = utils::trim(ip);
  if (tun.empty() || ip.empty())
    return false;

  if (interface_name)
    *interface_name = tun;
  if (internal_ip)
    *internal_ip = ip;
  return true;
}

static void clear_runtime_state() {
  remove_pid();
  remove_supervisor_pid();
  remove_route_ready();
}

static bool is_process_alive(pid_t pid) {
  if (pid <= 0)
    return false;
  if (kill(pid, 0) == 0)
    return true;
  return errno == EPERM;
}

static pid_t find_openconnect_pid() {
  std::string output = utils::run_command_output("pgrep -x openconnect");
  output = utils::trim(output);
  if (output.empty())
    return -1;
  try {
    return static_cast<pid_t>(std::stoi(output));
  } catch (...) {
    return -1;
  }
}

static void handle_supervisor_signal(int) {
  supervisor_stop_requested = 1;
  pid_t child_pid = supervisor_child_pid;
  if (child_pid > 0)
    kill(child_pid, SIGTERM);
}

static std::string describe_retry_policy(int retry_limit) {
  return retry_limit < 0 ? std::string("infinite")
                         : std::to_string(retry_limit) + " retries";
}

static std::string build_openconnect_command(const Config &cfg,
                                             const std::string &password) {
  std::ostringstream cmd;
  std::string openconnect_path = utils::get_openconnect_path();
  std::string heredoc_marker = "__EXV_PASSWORD_EOF__";
  while (password.find(heredoc_marker) != std::string::npos) {
    heredoc_marker += "_X";
  }

  cmd << "exec "
      << utils::shell_quote(openconnect_path.empty() ? std::string("openconnect")
                                                     : openconnect_path)
      << " " << utils::shell_quote(cfg.server)
      << " --useragent "
      << utils::shell_quote(cfg.useragent) << " -m " << cfg.mtu << " -u "
      << utils::shell_quote(cfg.username) << " --passwd-on-stdin"
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

static int run_supervisor(const Config &cfg, const std::string &password,
                          int retry_limit) {
  signal(SIGTERM, handle_supervisor_signal);
  signal(SIGINT, handle_supervisor_signal);

  logger::info("Reconnect supervisor started, retry policy: " +
               describe_retry_policy(retry_limit));

  bool first_attempt = true;
  int reconnect_attempts_used = 0;
  bool retry_limit_reached = false;

  while (!supervisor_stop_requested) {
    if (!first_attempt) {
      if (retry_limit == 0)
        break;
      if (retry_limit > -1 && reconnect_attempts_used >= retry_limit) {
        retry_limit_reached = true;
        break;
      }

      ++reconnect_attempts_used;
      logger::warn("VPN disconnected, attempting reconnect " +
                   std::to_string(reconnect_attempts_used) +
                   (retry_limit > -1 ? ("/" + std::to_string(retry_limit))
                                     : " (infinite mode)"));
      sleep(2);
      if (supervisor_stop_requested)
        break;
    }

    pid_t child_pid = fork();
    if (child_pid < 0) {
      logger::error("Failed to fork openconnect child process");
      if (first_attempt) {
        clear_runtime_state();
        return 1;
      }
      continue;
    }

    if (child_pid == 0) {
      std::string cmd = build_openconnect_command(cfg, password);
      execl("/bin/sh", "sh", "-c", cmd.c_str(), static_cast<char *>(nullptr));
      _exit(127);
    }

    supervisor_child_pid = child_pid;
    write_pid(child_pid);
    logger::info(std::string(first_attempt ? "Starting" : "Reconnect attempt") +
                 " openconnect, PID: " + std::to_string(child_pid));

    int wait_status = 0;
    bool route_ready_logged = false;
    while (true) {
      pid_t wait_result = waitpid(child_pid, &wait_status, WNOHANG);
      if (wait_result == child_pid)
        break;
      if (wait_result < 0) {
        if (errno == EINTR)
          continue;
        logger::error("waitpid failed while supervising openconnect");
        break;
      }

      if (!route_ready_logged) {
        pid_t vpn_pid = read_pid();
        std::string vpn_interface;
        std::string internal_ip;
        if (vpn_pid > 0 && is_process_alive(vpn_pid) &&
            read_route_ready(&vpn_interface, &internal_ip)) {
          logger::info(std::string(first_attempt
                                       ? "VPN connection ready under reconnect supervisor"
                                       : "VPN reconnect succeeded") +
                       ", PID: " + std::to_string(vpn_pid) +
                       ", interface: " + vpn_interface +
                       ", internal IP: " + internal_ip);
          route_ready_logged = true;
        }
      }

      usleep(250000);
    }

    if (!route_ready_logged) {
      std::string vpn_interface;
      std::string internal_ip;
      pid_t vpn_pid = read_pid();
      if (vpn_pid > 0 && is_process_alive(vpn_pid) &&
          read_route_ready(&vpn_interface, &internal_ip)) {
        logger::info(std::string(first_attempt
                                     ? "VPN connection ready under reconnect supervisor"
                                     : "VPN reconnect succeeded") +
                     ", PID: " + std::to_string(vpn_pid) +
                     ", interface: " + vpn_interface +
                     ", internal IP: " + internal_ip);
      }
    }

    supervisor_child_pid = -1;
    remove_pid();
    remove_route_ready();

    if (supervisor_stop_requested)
      break;

    if (WIFEXITED(wait_status)) {
      logger::warn("openconnect exited with code: " +
                   std::to_string(WEXITSTATUS(wait_status)));
    } else if (WIFSIGNALED(wait_status)) {
      logger::warn("openconnect terminated by signal: " +
                   std::to_string(WTERMSIG(wait_status)));
    }

    first_attempt = false;
  }

  if (retry_limit_reached) {
    logger::warn("Reconnect supervisor stopped after reaching retry limit: " +
                 std::to_string(retry_limit));
  } else if (supervisor_stop_requested) {
    logger::info("Reconnect supervisor stop requested");
  }

  clear_runtime_state();
  return 0;
}

int start(const Config &cfg, int retry_limit) {
  utils::print_header("EXV Starting");

  // ── Pre-flight checks ──────────────────────────────────────
  if (!utils::check_openconnect()) {
    utils::print_warning("openconnect is not installed.");
    std::cout << std::endl;
    std::cout << utils::BOLD << "  Install openconnect now? [Y/n] "
              << utils::RESET;
    std::string answer;
    std::getline(std::cin, answer);
    // Default to yes on empty input
    if (!answer.empty() && answer[0] != 'y' && answer[0] != 'Y') {
      utils::print_info("Aborting. Install openconnect manually and retry.");
      return 1;
    }

#ifdef __APPLE__
    // Check if Homebrew is available
    if (utils::run_command("which brew > /dev/null 2>&1") != 0) {
      utils::print_error("Homebrew is not installed either.");
      std::cout << std::endl;
      std::cout << utils::BOLD
                << "  Please run the following commands to install:"
                << utils::RESET << std::endl;
      std::cout << std::endl;
      std::cout << utils::YELLOW << "  # 1. Install Homebrew:" << utils::RESET
                << std::endl
                << "  /bin/bash -c \"$(curl -fsSL "
                   "https://raw.githubusercontent.com/Homebrew/install/HEAD/"
                   "install.sh)\""
                << std::endl
                << std::endl;
      std::cout << utils::YELLOW
                << "  # 2. Install openconnect:" << utils::RESET << std::endl
                << "  brew install openconnect" << std::endl
                << std::endl;
      logger::error("openconnect and Homebrew both not found");
      return 1;
    }

    // Homebrew found — attempt to install openconnect
    utils::print_info("Running: brew install openconnect ...");
    std::cout << std::endl;
    int ret = utils::run_command("brew install openconnect");
    std::cout << std::endl;
    if (ret != 0 || !utils::check_openconnect()) {
      utils::print_error("brew install openconnect failed.");
      utils::print_info("Try running it manually: brew install openconnect");
      logger::error("brew install openconnect failed");
      return 1;
    }
    utils::print_success("openconnect installed successfully!");
    logger::info("openconnect installed via Homebrew");
#elif defined(_WIN32)
    std::cout << "Download openconnect from: https://github.com/openconnect/openconnect-gui/releases\n";
    logger::error("openconnect not found");
    return 1;
#else
    // Check if Homebrew is available
    if (utils::run_command("which brew > /dev/null 2>&1") != 0) {
      utils::print_error("Homebrew is not installed either.");
      std::cout << std::endl;
      std::cout << utils::BOLD
                << "  Please run the following commands to install:"
                << utils::RESET << std::endl;
      std::cout << std::endl;
      std::cout << utils::YELLOW << "  # 1. Install Homebrew:" << utils::RESET
                << std::endl
                << "  /bin/bash -c \"$(curl -fsSL "
                   "https://raw.githubusercontent.com/Homebrew/install/HEAD/"
                   "install.sh)\""
                << std::endl
                << std::endl;
      std::cout << utils::YELLOW
                << "  # 2. Install openconnect:" << utils::RESET << std::endl
                << "  brew install openconnect" << std::endl
                << std::endl;
      logger::error("openconnect and Homebrew both not found");
      return 1;
    }

    // Homebrew found — attempt to install openconnect
    utils::print_info("Running: brew install openconnect ...");
    std::cout << std::endl;
    int ret = utils::run_command("brew install openconnect");
    std::cout << std::endl;
    if (ret != 0 || !utils::check_openconnect()) {
      utils::print_error("brew install openconnect failed.");
      utils::print_info("Try running it manually: brew install openconnect");
      logger::error("brew install openconnect failed");
      return 1;
    }
    utils::print_success("openconnect installed successfully!");
    logger::info("openconnect installed via Homebrew");
#endif
  }

  // Prefer helper daemon even when running as root — it manages session state
  // and the reconnect supervisor, and ensures stop works correctly.
  if (helper::is_available()) {
    Config validated_cfg = cfg;
    if (validated_cfg.username.empty()) {
      utils::print_error("Username not configured!");
      utils::print_info("Use 'exv config set username <your_username>' to set it.");
      return 1;
    }
    if (validated_cfg.remember_password && validated_cfg.password.empty()) {
      utils::print_error("Password not configured!");
      utils::print_info("Run: exv config set password");
      return 1;
    }
    if (validated_cfg.server.empty()) {
      utils::print_error("Server not configured!");
      return 1;
    }

    std::string plaintext_password = config::get_plaintext_password(validated_cfg);
    if (plaintext_password.empty()) {
      return 1;
    }

    return helper::start_via_helper(validated_cfg, plaintext_password, retry_limit)
               ? 0
               : 1;
  }

  if (!utils::check_root()) {
    utils::print_error("Root privileges required to start the VPN. Install the helper with 'sudo exv service install' or run with sudo.");
    logger::error("Not running as root for VPN start and helper is unavailable");
    return 1;
  }

  // Check if VPN is already running
  pid_t supervisor_pid = read_supervisor_pid();
  if (supervisor_pid > 0 && !is_process_alive(supervisor_pid)) {
    clear_runtime_state();
    supervisor_pid = -1;
  }

  pid_t existing_pid = read_pid();
  if (existing_pid <= 0 || !is_process_alive(existing_pid)) {
    existing_pid = find_openconnect_pid();
  }

  if (supervisor_pid > 0 || existing_pid > 0) {
    utils::print_warning(
        "VPN is already running (PID: " +
        std::to_string(existing_pid > 0 ? existing_pid : supervisor_pid) + ")");
    utils::print_info("Use 'exv stop' to stop the current connection first.");
    return 1;
  }

  // Validate config
  if (cfg.username.empty()) {
    utils::print_error("Username not configured!");
    utils::print_info("Use 'exv config set username <your_username>' to set it.");
    return 1;
  }
  // Only check for stored ciphertext when remember_password is true.
  // When false, get_plaintext_password() will prompt interactively.
  if (cfg.remember_password && cfg.password.empty()) {
    utils::print_error("Password not configured!");
    utils::print_info("Run: exv config set password");
    return 1;
  }
  if (cfg.server.empty()) {
    utils::print_error("Server not configured!");
    return 1;
  }

  // ── Decrypt / prompt password ───────────────────────────────
  std::string plaintext_password = config::get_plaintext_password(cfg);
  if (plaintext_password.empty()) {
    return 1; // error already printed by get_plaintext_password
  }

  return start_with_password(cfg, plaintext_password, retry_limit);
}

int start_with_password(const Config &cfg, const std::string &plaintext_password,
                        int retry_limit) {
  std::string openconnect_path = utils::get_openconnect_path();
  if (openconnect_path.empty()) {
    utils::print_error("openconnect is not installed or is not reachable by the current execution environment.");
    utils::print_info("Install openconnect or ensure it is available at a stable path such as /opt/homebrew/bin/openconnect.");
    logger::error("openconnect binary could not be resolved for VPN start");
    return 1;
  }

  // ── Generate tunnel script ─────────────────────────────────
  utils::print_info("Generating tunnel script...");
  if (!tunnel::write_script(cfg)) {
    return 1;
  }
  utils::print_success("Tunnel script ready (" +
                       std::to_string(cfg.routes.size()) + " routes)");

  if (!utils::check_root()) {
    utils::print_error("Root privileges required to start the VPN. Please run with sudo.");
    logger::error("Not running as root for VPN start");
    return 1;
  }

  utils::print_info("Connecting to " + cfg.server + " ...");
  logger::info("Starting VPN: " + cfg.server + " user=" + cfg.username);

  clear_runtime_state();
  supervisor_stop_requested = 0;
  supervisor_child_pid = -1;

  bool use_supervisor = retry_limit != 0;
  pid_t supervisor_pid = -1;

  if (!use_supervisor) {
    pid_t child_pid = fork();
    if (child_pid < 0) {
      utils::print_error("Failed to launch openconnect process.");
      logger::error("Failed to fork openconnect process");
      return 1;
    }

    if (child_pid == 0) {
      std::string cmd = build_openconnect_command(cfg, plaintext_password);
      execl("/bin/sh", "sh", "-c", cmd.c_str(), static_cast<char *>(nullptr));
      _exit(127);
    }

    write_pid(child_pid);

    pid_t vpn_pid = -1;
    std::string vpn_interface;
    std::string internal_ip;
    bool route_ready = false;
    for (int i = 0; i < 240; ++i) {
      usleep(250000);
      vpn_pid = read_pid();
      route_ready = read_route_ready(&vpn_interface, &internal_ip);

      if (vpn_pid > 0 && !is_process_alive(vpn_pid)) {
        clear_runtime_state();
        utils::print_error("Failed to establish the VPN connection.");
        utils::print_info("Check logs with: exv logs");
        logger::error("openconnect exited before initial connection was established");
        return 1;
      }

      if (vpn_pid > 0 && is_process_alive(vpn_pid) && route_ready)
        break;
    }

    if (vpn_pid > 0 && route_ready) {
      std::cout << std::endl;
      utils::print_success("VPN connected successfully!");
      std::cout << utils::DIM << "  PID: " << vpn_pid << utils::RESET
                << std::endl;
      std::cout << utils::DIM << "  Interface: " << vpn_interface << utils::RESET
                << std::endl;
      std::cout << utils::DIM << "  Internal IP: " << internal_ip
                << utils::RESET << std::endl;
      std::cout << utils::DIM << "  Server: " << cfg.server << utils::RESET
                << std::endl;
      std::cout << utils::DIM << "  Routes: " << cfg.routes.size()
                << " configured" << utils::RESET << std::endl;
      std::cout << std::endl;
      utils::print_info("Stop with: sudo exv stop");
      logger::info("VPN started, PID: " + std::to_string(vpn_pid));
      return 0;
    }

    utils::print_warning("VPN process started, but network routes are not ready yet.");
    utils::print_info("Check status with: exv status");
    utils::print_info("Check logs with: exv logs");
    logger::warn("VPN started without supervisor but route-ready marker not yet detected");
    return 0;
  }

  supervisor_pid = fork();
  if (supervisor_pid < 0) {
    utils::print_error("Failed to launch reconnect supervisor.");
    logger::error("Failed to fork reconnect supervisor");
    return 1;
  }

  if (supervisor_pid == 0) {
    if (setsid() < 0) {
      logger::error("Failed to detach reconnect supervisor session");
      _exit(1);
    }
    write_supervisor_pid(getpid());
    int result = run_supervisor(cfg, plaintext_password, retry_limit);
    _exit(result == 0 ? 0 : 1);
  }

  write_supervisor_pid(supervisor_pid);

  pid_t vpn_pid = -1;
  std::string vpn_interface;
  std::string internal_ip;
  bool route_ready = false;
  for (int i = 0; i < 20; ++i) {
    usleep(250000);
    vpn_pid = read_pid();
    route_ready = read_route_ready(&vpn_interface, &internal_ip);
    if (vpn_pid > 0 && is_process_alive(vpn_pid) && route_ready)
      break;
    if (!is_process_alive(supervisor_pid)) {
      clear_runtime_state();
      utils::print_error("Failed to establish the initial VPN connection.");
      utils::print_info("Check logs with: exv logs");
      logger::error("Reconnect supervisor exited before initial connection was established");
      return 1;
    }
  }

  // Verify connection
  if (vpn_pid > 0 && route_ready) {
    std::cout << std::endl;
    utils::print_success("VPN connected successfully!");
    std::cout << utils::DIM << "  PID: " << vpn_pid << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Supervisor PID: " << supervisor_pid
              << utils::RESET << std::endl;
    std::cout << utils::DIM << "  Interface: " << vpn_interface << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Internal IP: " << internal_ip
              << utils::RESET << std::endl;
    std::cout << utils::DIM << "  Server: " << cfg.server << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Routes: " << cfg.routes.size()
              << " configured" << utils::RESET << std::endl;
    if (retry_limit != 0) {
      std::cout << utils::DIM << "  Auto-reconnect: "
                << (retry_limit < 0 ? std::string("infinite")
                                    : std::to_string(retry_limit) + " retries")
                << utils::RESET << std::endl;
    }
    std::cout << std::endl;
    utils::print_info("Stop with: sudo exv stop");
    logger::info("VPN started, PID: " + std::to_string(vpn_pid) +
                 ", supervisor PID: " + std::to_string(supervisor_pid));
  } else {
    if (vpn_pid > 0 && !is_process_alive(vpn_pid))
      vpn_pid = -1;

    utils::print_warning("VPN process started, but network routes are not ready yet.");
    if (vpn_pid > 0) {
      std::cout << utils::DIM << "  PID: " << vpn_pid << utils::RESET
                << std::endl;
    }
    std::cout << utils::DIM << "  Supervisor PID: " << supervisor_pid
              << utils::RESET << std::endl;
    std::cout << utils::DIM << "  Server: " << cfg.server << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Routes: " << cfg.routes.size()
              << " configured" << utils::RESET << std::endl;
    std::cout << utils::DIM << "  Auto-reconnect: "
              << (retry_limit < 0 ? std::string("infinite")
                                  : std::to_string(retry_limit) + " retries")
              << utils::RESET << std::endl;
    std::cout << std::endl;
    utils::print_info("Check status with: exv status");
    utils::print_info("Check logs with: exv logs");
    utils::print_info("Stop with: sudo exv stop");
    logger::warn("VPN supervisor started and returned before route-ready marker was detected");
  }

  return 0;
}

int stop() {
  utils::print_header("EXV Stopping");

  // Prefer helper daemon even when running as root — it manages session state
  // and the reconnect supervisor. Direct kills leave stale state and the
  // supervisor respawns openconnect immediately.
  if (helper::is_available()) {
    bool ok = helper::stop_via_helper();
    if (ok) return 0;
    // Helper said not running — check for orphaned processes before giving up
  }

  if (!utils::check_root()) {
    utils::print_error("Root privileges required. Please run with sudo.");
    return 1;
  }

  pid_t supervisor_pid = read_supervisor_pid();
  if (supervisor_pid > 0 && !is_process_alive(supervisor_pid)) {
    remove_supervisor_pid();
    supervisor_pid = -1;
  }

  // Find openconnect process
  pid_t pid = read_pid();
  if (pid <= 0 || !is_process_alive(pid)) {
    // Try pgrep as fallback
    pid = find_openconnect_pid();
  }

  if (pid <= 0 && supervisor_pid <= 0) {
    utils::print_error("No openconnect process found. VPN is not running.");
    clear_runtime_state();
    logger::info("Stop requested but no VPN process found");
    return 1;
  }

  if (pid > 0) {
    utils::print_info("Found openconnect process: PID " + std::to_string(pid));
  }
  if (supervisor_pid > 0) {
    utils::print_info("Found reconnect supervisor: PID " +
                      std::to_string(supervisor_pid));
  }
  utils::print_info("Sending SIGTERM...");
  logger::info("Stopping VPN, PID: " + std::to_string(pid) +
               ", supervisor PID: " + std::to_string(supervisor_pid));

  if (supervisor_pid > 0)
    kill(supervisor_pid, SIGTERM);
  if (pid > 0)
    kill(pid, SIGTERM);

  for (int i = 0; i < 10; ++i) {
    usleep(300000);
    if ((pid <= 0 || !is_process_alive(pid)) &&
        (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))) {
      break;
    }
  }

  if (pid > 0 && is_process_alive(pid)) {
    utils::print_warning("openconnect still running, sending SIGKILL...");
    kill(pid, SIGKILL);
  }
  if (supervisor_pid > 0 && is_process_alive(supervisor_pid)) {
    utils::print_warning("Reconnect supervisor still running, sending SIGKILL...");
    kill(supervisor_pid, SIGKILL);
  }

  usleep(500000);

  pid_t remaining_pid = find_openconnect_pid();
  if (remaining_pid > 0 && remaining_pid != pid) {
    utils::print_warning("Detected remaining openconnect process, sending SIGKILL...");
    kill(remaining_pid, SIGKILL);
    usleep(500000);
  }

  if ((pid > 0 && is_process_alive(pid)) ||
      (remaining_pid > 0 && is_process_alive(remaining_pid)) ||
      (supervisor_pid > 0 && is_process_alive(supervisor_pid))) {
    utils::print_error("Failed to stop VPN connection!");
    logger::error("Failed to stop VPN, PID: " + std::to_string(pid) +
                  ", remaining PID: " + std::to_string(remaining_pid) +
                  ", supervisor PID: " + std::to_string(supervisor_pid));
    return 1;
  }

  clear_runtime_state();
  std::cout << std::endl;
  utils::print_success("VPN connection stopped successfully! 🎉");
  logger::info("VPN stopped successfully");
  return 0;
}

int status() {
  utils::print_header("EXV Status");

  if (helper::is_available()) {
    return helper::show_status_via_helper() ? 0 : 1;
  }

  pid_t supervisor_pid = read_supervisor_pid();
  bool supervisor_from_pidfile = true;
  if (supervisor_pid <= 0 || !is_process_alive(supervisor_pid)) {
    supervisor_pid = -1;
    supervisor_from_pidfile = false;
  }

  pid_t pid = read_pid();
  bool from_pidfile = true;
  if (pid <= 0 || !is_process_alive(pid)) {
    pid = find_openconnect_pid();
    from_pidfile = false;
  }

  std::string vpn_interface;
  std::string internal_ip;
  bool route_ready = read_route_ready(&vpn_interface, &internal_ip);

  if (pid > 0 || supervisor_pid > 0) {
    std::cout << utils::GREEN << utils::BOLD << "  ● VPN is RUNNING"
              << utils::RESET << std::endl;
    std::cout << std::endl;
    if (pid > 0) {
      std::cout << "  PID            : " << pid;
      if (!from_pidfile)
        std::cout << "  (detected via pgrep)";
      std::cout << std::endl;
    }
    if (supervisor_pid > 0) {
      std::cout << "  Supervisor PID : " << supervisor_pid;
      if (!supervisor_from_pidfile)
        std::cout << "  (detected outside pidfile)";
      std::cout << std::endl;
    }
    std::cout << "  Network Ready  : "
              << (route_ready ? "yes" : "no (waiting for tunnel script)")
              << std::endl;
    if (route_ready) {
      std::cout << "  Interface      : " << vpn_interface << std::endl;
      std::cout << "  Internal IP    : " << internal_ip << std::endl;
    }

    // Try to get tunnel interface info
#ifdef __APPLE__
    std::string ifconfig_out =
        utils::run_command_output("ifconfig | grep -A 2 'utun' | head -20");
#elif defined(_WIN32)
    std::string ifconfig_out =
        utils::run_command_output("netsh interface show interface 2>nul");
#else
    std::string ifconfig_out =
        utils::run_command_output("ip addr show type tun 2>/dev/null | head -20");
#endif
    if (!ifconfig_out.empty()) {
      std::cout << std::endl;
      std::cout << utils::DIM << "  Network Interfaces:" << utils::RESET
                << std::endl;
      // Print each line indented
      std::istringstream iss(ifconfig_out);
      std::string line;
      while (std::getline(iss, line)) {
        std::cout << "    " << line << std::endl;
      }
    }
  } else {
    std::cout << utils::RED << utils::BOLD << "  ● VPN is NOT RUNNING"
              << utils::RESET << std::endl;
    clear_runtime_state();
  }

  std::cout << std::endl;
  return 0;
}

} // namespace vpn
} // namespace ecnuvpn
