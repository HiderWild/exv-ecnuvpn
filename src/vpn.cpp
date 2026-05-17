#include "vpn.hpp"
#include "config.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "tunnel.hpp"
#include "utils.hpp"
#include "virtual_network.hpp"

#include <cerrno>
#include <csignal>
#include <filesystem>
#include <functional>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <sstream>
#include <vector>
#ifndef _WIN32
#include <sys/wait.h>
#include <unistd.h>
#else
#include <windows.h>
#ifdef _MSC_VER
typedef int pid_t;
#endif
#endif

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
#ifndef _WIN32
  if (kill(pid, 0) == 0)
    return true;
  return errno == EPERM;
#else
  HANDLE hProcess = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE,
                                static_cast<DWORD>(pid));
  if (!hProcess)
    return false;
  DWORD exitCode = 0;
  BOOL ok = GetExitCodeProcess(hProcess, &exitCode);
  CloseHandle(hProcess);
  return ok && exitCode == STILL_ACTIVE;
#endif
}

static pid_t find_openconnect_pid() {
#ifndef _WIN32
  std::string output = utils::run_command_output("pgrep -x openconnect");
  output = utils::trim(output);
  if (output.empty())
    return -1;
  try {
    return static_cast<pid_t>(std::stoi(output));
  } catch (...) {
    return -1;
  }
#else
  std::string output = utils::run_command_output(
      "tasklist /FI \"IMAGENAME eq openconnect.exe\" /NH /FO CSV 2>nul");
  // Parse CSV: "openconnect.exe","1234","Console","1","4,096 K"
  auto start = output.find('"', output.find(',') + 1);
  if (start == std::string::npos)
    return -1;
  auto end = output.find('"', start + 1);
  if (end == std::string::npos)
    return -1;
  std::string pid_str = output.substr(start + 1, end - start - 1);
  try {
    return static_cast<pid_t>(std::stoi(pid_str));
  } catch (...) {
    return -1;
  }
#endif
}

static void handle_supervisor_signal(int) {
  supervisor_stop_requested = 1;
  pid_t child_pid = supervisor_child_pid;
  if (child_pid > 0) {
#ifndef _WIN32
    kill(child_pid, SIGTERM);
#else
    HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE,
                           static_cast<DWORD>(child_pid));
    if (h) {
      TerminateProcess(h, 1);
      CloseHandle(h);
    }
#endif
  }
}

static std::string describe_retry_policy(int retry_limit) {
  return retry_limit < 0 ? std::string("infinite")
                         : std::to_string(retry_limit) + " retries";
}

#ifdef _WIN32
static std::string windows_generated_interface_name(const Config &cfg) {
  std::ostringstream seed;
  seed << cfg.username << "@" << cfg.server;
  std::size_t hash = std::hash<std::string>{}(seed.str());
  std::ostringstream name;
  name << "ECNUVPN-" << std::uppercase << std::hex
       << static_cast<unsigned long long>(hash & 0xffffffffULL);
  return name.str();
}

static std::string select_windows_interface_name(const Config &cfg) {
  if (cfg.windows_tunnel_driver == "tap")
    return cfg.windows_tap_interface;

  if (cfg.windows_tunnel_driver == "wintun")
    return windows_generated_interface_name(cfg);

  if (!utils::get_bundled_wintun_path().empty())
    return windows_generated_interface_name(cfg);

  return cfg.windows_tap_interface;
}
#endif

#ifndef _WIN32
static std::string build_openconnect_command(const Config &cfg,
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
#else
static std::string windows_quote_arg(const std::string &value) {
  if (value.empty())
    return "\"\"";
  bool needs_quotes = value.find_first_of(" 	\"") != std::string::npos;
  if (!needs_quotes)
    return value;

  std::string quoted = "\"";
  unsigned int backslashes = 0;
  for (char c : value) {
    if (c == '\\') {
      ++backslashes;
      continue;
    }
    if (c == '"') {
      quoted.append(backslashes * 2 + 1, '\\');
      quoted.push_back('"');
      backslashes = 0;
      continue;
    }
    quoted.append(backslashes, '\\');
    backslashes = 0;
    quoted.push_back(c);
  }
  quoted.append(backslashes * 2, '\\');
  quoted.push_back('"');
  return quoted;
}

static std::string build_openconnect_command_line(const Config &cfg) {
  std::vector<std::string> args;
  std::string openconnect_path = utils::get_openconnect_path(cfg.openconnect_runtime);
  args.push_back(openconnect_path.empty() ? std::string("openconnect.exe")
                                    : openconnect_path);
  args.push_back(cfg.server);
  args.push_back("--useragent");
  args.push_back(cfg.useragent);
  args.push_back("-m");
  args.push_back(std::to_string(cfg.mtu));
  args.push_back("-u");
  args.push_back(cfg.username);
  args.push_back("--passwd-on-stdin");
  args.push_back("--script");
  args.push_back(utils::get_tunnel_path());
  std::string interface_name = select_windows_interface_name(cfg);
  if (!interface_name.empty()) {
    args.push_back("--interface");
    args.push_back(interface_name);
  }
  if (cfg.disable_dtls) {
    args.push_back("--no-dtls");
  }
  for (const auto &arg : cfg.extra_args) {
    args.push_back(arg);
  }

  std::ostringstream cmd;
  for (std::size_t i = 0; i < args.size(); ++i) {
    if (i)
      cmd << ' ';
    cmd << windows_quote_arg(args[i]);
  }
  return cmd.str();
}

static HANDLE open_inheritable_append_handle(const std::string &path) {
  std::error_code ec;
  std::filesystem::path parent = std::filesystem::path(path).parent_path();
  if (!parent.empty())
    std::filesystem::create_directories(parent, ec);

  SECURITY_ATTRIBUTES sa = {};
  sa.nLength = sizeof(sa);
  sa.bInheritHandle = TRUE;
  HANDLE h = CreateFileA(path.c_str(), FILE_APPEND_DATA | GENERIC_WRITE,
                         FILE_SHARE_READ | FILE_SHARE_WRITE, &sa, OPEN_ALWAYS,
                         FILE_ATTRIBUTE_NORMAL, NULL);
  if (h != INVALID_HANDLE_VALUE)
    SetFilePointer(h, 0, NULL, FILE_END);
  return h;
}

static HANDLE open_inheritable_null_handle(DWORD access) {
  SECURITY_ATTRIBUTES sa = {};
  sa.nLength = sizeof(sa);
  sa.bInheritHandle = TRUE;
  return CreateFileA("NUL", access, FILE_SHARE_READ | FILE_SHARE_WRITE, &sa,
                     OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
}

static bool launch_openconnect_process(const Config &cfg,
                                       const std::string &password,
                                       pid_t *pid,
                                       HANDLE *process_handle) {
  if (pid)
    *pid = -1;
  if (process_handle)
    *process_handle = NULL;

  SECURITY_ATTRIBUTES sa = {};
  sa.nLength = sizeof(sa);
  sa.bInheritHandle = TRUE;
  HANDLE stdin_read = NULL;
  HANDLE stdin_write = NULL;
  if (!CreatePipe(&stdin_read, &stdin_write, &sa, 0)) {
    logger::error("Failed to create password pipe for openconnect.");
    return false;
  }
  SetHandleInformation(stdin_write, HANDLE_FLAG_INHERIT, 0);

  std::string log_path = utils::expand_home(cfg.log_file);
  HANDLE log_handle = open_inheritable_append_handle(log_path);
  if (log_handle == INVALID_HANDLE_VALUE) {
    CloseHandle(stdin_read);
    CloseHandle(stdin_write);
    logger::error("Failed to open VPN log file for Windows launch: " + log_path);
    return false;
  }

  std::string openconnect_path = utils::get_openconnect_path(cfg.openconnect_runtime);
  if (openconnect_path.empty()) {
    CloseHandle(stdin_read);
    CloseHandle(stdin_write);
    CloseHandle(log_handle);
    logger::error("Bundled/system openconnect binary could not be resolved.");
    return false;
  }
  std::string cmdline = build_openconnect_command_line(cfg);
  std::vector<char> mutable_cmd(cmdline.begin(), cmdline.end());
  mutable_cmd.push_back('\0');
  std::string current_dir =
      std::filesystem::path(openconnect_path).parent_path().string();

  STARTUPINFOA si = {};
  si.cb = sizeof(si);
  si.dwFlags = STARTF_USESTDHANDLES;
  si.hStdInput = stdin_read;
  si.hStdOutput = log_handle;
  si.hStdError = log_handle;
  PROCESS_INFORMATION pi = {};
  BOOL created = CreateProcessA(openconnect_path.c_str(), mutable_cmd.data(),
                                NULL, NULL, TRUE, CREATE_NO_WINDOW, NULL,
                                current_dir.empty() ? NULL : current_dir.c_str(),
                                &si, &pi);
  CloseHandle(stdin_read);
  CloseHandle(log_handle);
  if (!created) {
    CloseHandle(stdin_write);
    logger::error("Failed to create openconnect process: " +
                  std::to_string(GetLastError()));
    return false;
  }

  std::string stdin_payload = password;
  stdin_payload.push_back('\n');
  DWORD written = 0;
  BOOL wrote = WriteFile(stdin_write, stdin_payload.data(),
                        static_cast<DWORD>(stdin_payload.size()), &written, NULL);
  CloseHandle(pi.hThread);
  if (!wrote) {
    CloseHandle(stdin_write);
    TerminateProcess(pi.hProcess, 1);
    CloseHandle(pi.hProcess);
    logger::error("Failed to write password to openconnect stdin.");
    return false;
  }

  // Keep the write end open while openconnect is running. On Windows,
  // openconnect later spawns the tunnel script; closing stdin immediately can
  // leave cscript.exe with an invalid inherited standard input handle.

  if (pid)
    *pid = static_cast<pid_t>(pi.dwProcessId);
  if (process_handle)
    *process_handle = pi.hProcess;
  else
    CloseHandle(pi.hProcess);
  return true;
}

static bool launch_supervisor_process(const Config &cfg,
                                      const std::string &password,
                                      int retry_limit,
                                      pid_t *pid) {
  if (pid)
    *pid = -1;

  nlohmann::json request{{"config", cfg},
                         {"password", password},
                         {"retry_limit", retry_limit},
                         {"home", utils::get_effective_home()},
                         {"config_dir", utils::get_config_dir()}};
  std::string payload = request.dump();

  SECURITY_ATTRIBUTES sa = {};
  sa.nLength = sizeof(sa);
  sa.bInheritHandle = TRUE;
  HANDLE stdin_read = NULL;
  HANDLE stdin_write = NULL;
  if (!CreatePipe(&stdin_read, &stdin_write, &sa, 0)) {
    logger::error("Failed to create Windows supervisor pipe.");
    return false;
  }
  SetHandleInformation(stdin_write, HANDLE_FLAG_INHERIT, 0);

  HANDLE null_handle = open_inheritable_null_handle(GENERIC_WRITE);
  if (null_handle == INVALID_HANDLE_VALUE) {
    CloseHandle(stdin_read);
    CloseHandle(stdin_write);
    logger::error("Failed to open NUL handle for Windows supervisor.");
    return false;
  }

  std::string exec_path = utils::get_executable_path();
  std::string cmdline = windows_quote_arg(exec_path) + " __vpn-supervisor";
  std::vector<char> mutable_cmd(cmdline.begin(), cmdline.end());
  mutable_cmd.push_back('\0');

  STARTUPINFOA si = {};
  si.cb = sizeof(si);
  si.dwFlags = STARTF_USESTDHANDLES;
  si.hStdInput = stdin_read;
  si.hStdOutput = null_handle;
  si.hStdError = null_handle;
  PROCESS_INFORMATION pi = {};
  BOOL created = CreateProcessA(exec_path.c_str(), mutable_cmd.data(), NULL,
                                NULL, TRUE, CREATE_NO_WINDOW, NULL, NULL,
                                &si, &pi);
  CloseHandle(stdin_read);
  CloseHandle(null_handle);
  if (!created) {
    CloseHandle(stdin_write);
    logger::error("Failed to create reconnect supervisor process: " +
                  std::to_string(GetLastError()));
    return false;
  }

  DWORD written = 0;
  BOOL wrote = WriteFile(stdin_write, payload.data(),
                        static_cast<DWORD>(payload.size()), &written, NULL);
  CloseHandle(stdin_write);
  CloseHandle(pi.hThread);
  if (!wrote) {
    TerminateProcess(pi.hProcess, 1);
    CloseHandle(pi.hProcess);
    logger::error("Failed to send startup payload to Windows reconnect supervisor.");
    return false;
  }

  if (pid)
    *pid = static_cast<pid_t>(pi.dwProcessId);
  CloseHandle(pi.hProcess);
  return true;
}
#endif

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
#ifndef _WIN32
      sleep(2);
#else
      Sleep(2000);
#endif
      if (supervisor_stop_requested)
        break;
    }

    pid_t child_pid = -1;
#ifndef _WIN32
    child_pid = fork();
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
#else
    HANDLE child_handle = NULL;
    if (!launch_openconnect_process(cfg, password, &child_pid, &child_handle)) {
      if (first_attempt) {
        clear_runtime_state();
        return 1;
      }
      continue;
    }
#endif

    supervisor_child_pid = child_pid;
    write_pid(child_pid);
    logger::info(std::string(first_attempt ? "Starting" : "Reconnect attempt") +
                 " openconnect, PID: " + std::to_string(child_pid));

    int wait_status = 0;
    bool route_ready_logged = false;
#ifndef _WIN32
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
#else
    DWORD child_exit_code = 1;
    HANDLE hChild = child_handle;
    while (true) {
      if (!hChild) {
        logger::error("Invalid Windows openconnect process handle while supervising.");
        break;
      }
      DWORD wait_result = WaitForSingleObject(hChild, 250);
      if (wait_result == WAIT_OBJECT_0) {
        break;
      }
      if (wait_result == WAIT_FAILED) {
        logger::error("WaitForSingleObject failed while supervising openconnect");
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
    }
    if (hChild) {
      GetExitCodeProcess(hChild, &child_exit_code);
      CloseHandle(hChild);
    }
#endif

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

#ifndef _WIN32
    if (WIFEXITED(wait_status)) {
      logger::warn("openconnect exited with code: " +
                   std::to_string(WEXITSTATUS(wait_status)));
    } else if (WIFSIGNALED(wait_status)) {
      logger::warn("openconnect terminated by signal: " +
                   std::to_string(WTERMSIG(wait_status)));
    }
#else
    logger::warn("openconnect exited with code: " +
                 std::to_string(child_exit_code));
#endif

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

#ifdef _WIN32
int supervisor_main() {
  try {
    std::ostringstream payload;
    payload << std::cin.rdbuf();
    nlohmann::json request = nlohmann::json::parse(payload.str());
    std::string home = request.value("home", std::string());
    std::string config_dir = request.value("config_dir", std::string());
    utils::set_runtime_path_override(home, config_dir);
    logger::init();
    write_supervisor_pid(static_cast<pid_t>(GetCurrentProcessId()));
    Config cfg = request.at("config").get<Config>();
    std::string password = request.at("password").get<std::string>();
    int retry_limit = request.value("retry_limit", 0);
    return run_supervisor(cfg, password, retry_limit);
  } catch (...) {
    clear_runtime_state();
    return 1;
  }
}
#endif

int start(const Config &cfg, int retry_limit) {
  utils::print_header("EXV Starting");

  // ── Pre-flight checks ──────────────────────────────────────
  if (!utils::check_openconnect(cfg.openconnect_runtime)) {
#ifdef _WIN32
    if (cfg.openconnect_runtime != "system") {
      utils::print_error("Bundled OpenConnect runtime is missing from this installation.");
      utils::print_info("Rebuild the desktop package with the bundled native runtime assets.");
      logger::error("Bundled OpenConnect runtime missing on Windows");
      return 1;
    }
#elif defined(__APPLE__)
    if (cfg.openconnect_runtime != "system") {
      utils::print_error("Bundled OpenConnect runtime is missing from this installation.");
      utils::print_info("Rebuild the desktop package with the macOS bundled runtime assets.");
      logger::error("Bundled OpenConnect runtime missing on macOS");
      return 1;
    }
#endif
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
    if (ret != 0 || !utils::check_openconnect(cfg.openconnect_runtime)) {
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
    // Linux: try system package managers
    if (utils::run_command("which apt-get > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo apt-get install -y openconnect ...");
      int ret = utils::run_command("sudo apt-get install -y openconnect");
      if (ret != 0 || !utils::check_openconnect(cfg.openconnect_runtime)) {
        utils::print_error("apt-get install openconnect failed.");
        return 1;
      }
    } else if (utils::run_command("which dnf > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo dnf install -y openconnect ...");
      int ret = utils::run_command("sudo dnf install -y openconnect");
      if (ret != 0 || !utils::check_openconnect(cfg.openconnect_runtime)) {
        utils::print_error("dnf install openconnect failed.");
        return 1;
      }
    } else if (utils::run_command("which pacman > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo pacman -S --noconfirm openconnect ...");
      int ret = utils::run_command("sudo pacman -S --noconfirm openconnect");
      if (ret != 0 || !utils::check_openconnect(cfg.openconnect_runtime)) {
        utils::print_error("pacman install openconnect failed.");
        return 1;
      }
    } else {
      utils::print_error("No supported package manager found (apt-get/dnf/pacman).");
      utils::print_info("Install openconnect manually, then run exv again.");
      logger::error("openconnect not found, no package manager available");
      return 1;
    }
    utils::print_success("openconnect installed successfully!");
    logger::info("openconnect installed via system package manager");
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
#ifdef _WIN32
    utils::print_error("Administrator privileges required to start the VPN. Install the helper with 'exv service install' from an elevated prompt or run this command as Administrator.");
    logger::error("Not running as Administrator for VPN start and helper is unavailable");
#else
    utils::print_error("Root privileges required to start the VPN. Install the helper with 'sudo exv service install' or run with sudo.");
    logger::error("Not running as root for VPN start and helper is unavailable");
#endif
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
  std::string openconnect_path = utils::get_openconnect_path(cfg.openconnect_runtime);
  if (openconnect_path.empty()) {
    utils::print_error("OpenConnect is not reachable by the current execution environment.");
    if (cfg.openconnect_runtime == "system") {
      utils::print_info("Install system openconnect or switch the runtime mode back to bundled/auto.");
    } else {
      utils::print_info("Ensure the desktop package contains the bundled OpenConnect runtime assets.");
    }
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
#ifdef _WIN32
    utils::print_error("Administrator privileges required to start the VPN. Please run from an elevated prompt.");
    logger::error("Not running as Administrator for VPN start");
#else
    utils::print_error("Root privileges required to start the VPN. Please run with sudo.");
    logger::error("Not running as root for VPN start");
#endif
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
    pid_t child_pid = -1;
#ifndef _WIN32
    child_pid = fork();
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
#else
    HANDLE child_handle = NULL;
    if (!launch_openconnect_process(cfg, plaintext_password, &child_pid,
                                   &child_handle)) {
      utils::print_error("Failed to launch openconnect process.");
      logger::error("Failed to create openconnect process");
      return 1;
    }
    if (child_handle)
      CloseHandle(child_handle);
#endif

    write_pid(child_pid);

    pid_t vpn_pid = -1;
    std::string vpn_interface;
    std::string internal_ip;
    bool route_ready = false;
    for (int i = 0; i < 240; ++i) {
#ifndef _WIN32
      usleep(250000);
#else
      Sleep(250);
#endif
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
      virtual_network::print_notice(
          virtual_network::detect_upstream_virtual_network(vpn_interface));
      std::cout << std::endl;
#ifdef _WIN32
      utils::print_info("Stop with: exv stop");
#else
      utils::print_info("Stop with: sudo exv stop");
#endif
      logger::info("VPN started, PID: " + std::to_string(vpn_pid));
      return 0;
    }

    utils::print_warning("VPN process started, but network routes are not ready yet.");
    utils::print_info("Check status with: exv status");
    utils::print_info("Check logs with: exv logs");
    logger::warn("VPN started without supervisor but route-ready marker not yet detected");
    return 0;
  }

  supervisor_pid = -1;
#ifndef _WIN32
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
#else
  if (!launch_supervisor_process(cfg, plaintext_password, retry_limit,
                                 &supervisor_pid)) {
    utils::print_error("Failed to launch reconnect supervisor.");
    logger::error("Failed to create reconnect supervisor process");
    return 1;
  }
#endif

  write_supervisor_pid(supervisor_pid);

  pid_t vpn_pid = -1;
  std::string vpn_interface;
  std::string internal_ip;
  bool route_ready = false;
  for (int i = 0; i < 20; ++i) {
#ifndef _WIN32
    usleep(250000);
#else
    Sleep(250);
#endif
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
    virtual_network::print_notice(
        virtual_network::detect_upstream_virtual_network(vpn_interface));
    if (retry_limit != 0) {
      std::cout << utils::DIM << "  Auto-reconnect: "
                << (retry_limit < 0 ? std::string("infinite")
                                    : std::to_string(retry_limit) + " retries")
                << utils::RESET << std::endl;
    }
    std::cout << std::endl;
#ifdef _WIN32
    utils::print_info("Stop with: exv stop");
#else
    utils::print_info("Stop with: sudo exv stop");
#endif
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
#ifdef _WIN32
    utils::print_info("Stop with: exv stop");
#else
    utils::print_info("Stop with: sudo exv stop");
#endif
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
#ifdef _WIN32
    utils::print_error("Administrator privileges required. Please run from an elevated prompt.");
#else
    utils::print_error("Root privileges required. Please run with sudo.");
#endif
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

  // Clean up routes before killing openconnect — while the tunnel
  // interface is still valid, route deletion is more reliable.
#ifndef _WIN32
  tunnel::cleanup_routes();
#endif

#ifndef _WIN32
  if (supervisor_pid > 0)
    kill(supervisor_pid, SIGTERM);
  if (pid > 0)
    kill(pid, SIGTERM);
#else
  if (supervisor_pid > 0) {
    HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE,
                           static_cast<DWORD>(supervisor_pid));
    if (h) { TerminateProcess(h, 1); CloseHandle(h); }
  }
  if (pid > 0) {
    HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE,
                           static_cast<DWORD>(pid));
    if (h) { TerminateProcess(h, 1); CloseHandle(h); }
  }
#endif

  for (int i = 0; i < 10; ++i) {
#ifndef _WIN32
    usleep(300000);
#else
    Sleep(300);
#endif
    if ((pid <= 0 || !is_process_alive(pid)) &&
        (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))) {
      break;
    }
  }

  if (pid > 0 && is_process_alive(pid)) {
    utils::print_warning("openconnect still running, sending SIGKILL...");
#ifndef _WIN32
    kill(pid, SIGKILL);
#else
    HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE,
                           static_cast<DWORD>(pid));
    if (h) { TerminateProcess(h, 1); CloseHandle(h); }
#endif
  }
  if (supervisor_pid > 0 && is_process_alive(supervisor_pid)) {
    utils::print_warning("Reconnect supervisor still running, sending SIGKILL...");
#ifndef _WIN32
    kill(supervisor_pid, SIGKILL);
#else
    HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE,
                           static_cast<DWORD>(supervisor_pid));
    if (h) { TerminateProcess(h, 1); CloseHandle(h); }
#endif
  }

#ifndef _WIN32
  usleep(500000);
#else
  Sleep(500);
#endif

  pid_t remaining_pid = find_openconnect_pid();
  if (remaining_pid > 0 && remaining_pid != pid) {
    utils::print_warning("Detected remaining openconnect process, sending SIGKILL...");
#ifndef _WIN32
    kill(remaining_pid, SIGKILL);
    usleep(500000);
#else
    HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE,
                           static_cast<DWORD>(remaining_pid));
    if (h) { TerminateProcess(h, 1); CloseHandle(h); }
    Sleep(500);
#endif
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

nlohmann::json direct_status_json(const Config &cfg) {
  pid_t supervisor_pid = read_supervisor_pid();
  if (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))
    supervisor_pid = -1;

  pid_t pid = read_pid();
  if (pid <= 0 || !is_process_alive(pid))
    pid = find_openconnect_pid();

  std::string vpn_interface;
  std::string internal_ip;
  bool route_ready = read_route_ready(&vpn_interface, &internal_ip);

  bool connected = (pid > 0 || supervisor_pid > 0);

  nlohmann::json j;
  j["connected"] = connected;
  j["server"] = cfg.server;
  j["username"] = cfg.username;
  j["pid"] = pid > 0 ? pid : -1;
  j["supervisor_pid"] = supervisor_pid > 0 ? supervisor_pid : -1;
  j["network_ready"] = connected && route_ready;
  j["interface"] = route_ready ? vpn_interface : "";
  j["internal_ip"] = route_ready ? internal_ip : "";
  j["route_count"] = static_cast<int>(cfg.routes.size());
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = 0;
  j["tx_bytes"] = 0;
  j["mode"] = connected ? "direct" : "disconnected";
  j["log_path"] = cfg.log_file;
  virtual_network::add_status_fields(j, j.value("interface", std::string()));
  return j;
}

nlohmann::json direct_stop_json() {
  if (!utils::check_root()) {
#ifdef _WIN32
    return nlohmann::json{{"ok", false}, {"error_type", "elevation_required"},
                          {"message", "Administrator privileges required to stop VPN."},
                          {"recoverable", true},
                          {"recommended_action", "Install the helper service to avoid elevation prompts, or retry and accept the elevation request"}};
#else
    return nlohmann::json{{"ok", false}, {"error_type", "elevation_required"},
                          {"message", "Root privileges required to stop VPN."},
                          {"recoverable", true},
                          {"recommended_action", "Install the helper service to avoid elevation prompts, or run with sudo"}};
#endif
  }

  pid_t supervisor_pid = read_supervisor_pid();
  if (supervisor_pid > 0 && !is_process_alive(supervisor_pid)) {
    remove_supervisor_pid();
    supervisor_pid = -1;
  }

  pid_t pid = read_pid();
  if (pid <= 0 || !is_process_alive(pid))
    pid = find_openconnect_pid();

  if (pid <= 0 && supervisor_pid <= 0) {
    clear_runtime_state();
    return nlohmann::json{{"ok", true}, {"message", "VPN was not running."}};
  }

#ifndef _WIN32
  tunnel::cleanup_routes();
  if (supervisor_pid > 0) kill(supervisor_pid, SIGTERM);
  if (pid > 0) kill(pid, SIGTERM);
#else
  if (supervisor_pid > 0) {
    HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE,
                           static_cast<DWORD>(supervisor_pid));
    if (h) { TerminateProcess(h, 1); CloseHandle(h); }
  }
  if (pid > 0) {
    HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE,
                           static_cast<DWORD>(pid));
    if (h) { TerminateProcess(h, 1); CloseHandle(h); }
  }
#endif

  // Wait for processes to die
  for (int i = 0; i < 10; ++i) {
#ifndef _WIN32
    usleep(300000);
#else
    Sleep(300);
#endif
    pid_t check_pid = read_pid();
    if (check_pid <= 0 || !is_process_alive(check_pid)) {
      if (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))
        break;
    }
  }

  clear_runtime_state();
  logger::info("VPN stopped via direct mode (PID " + std::to_string(pid) +
               ", supervisor PID " + std::to_string(supervisor_pid) + ")");

  return nlohmann::json{{"ok", true}, {"message", "VPN stopped successfully."}};
}

nlohmann::json direct_start_json(const Config &cfg,
                                 const std::string &plaintext_password,
                                 int retry_limit) {
  if (!utils::check_root()) {
#ifdef _WIN32
    return nlohmann::json{{"ok", false}, {"error_type", "elevation_required"},
                          {"message", "Administrator privileges required to start VPN."},
                          {"recoverable", true},
                          {"recommended_action", "Install the helper service to avoid elevation prompts, or retry and accept the elevation request"}};
#else
    return nlohmann::json{{"ok", false}, {"error_type", "elevation_required"},
                          {"message", "Root privileges required to start VPN."},
                          {"recoverable", true},
                          {"recommended_action", "Install the helper service to avoid elevation prompts, or run with sudo"}};
#endif
  }

  std::string openconnect_path = utils::get_openconnect_path(cfg.openconnect_runtime);
  if (openconnect_path.empty()) {
    return nlohmann::json{{"ok", false}, {"error_type", "runtime_missing"},
                          {"message", "OpenConnect runtime is not available."},
                          {"recoverable", true},
                          {"recommended_action", "Ensure the desktop package contains the bundled OpenConnect runtime"}};
  }

  if (cfg.server.empty())
    return nlohmann::json{{"ok", false}, {"error_type", "config_invalid"},
                          {"message", "VPN server is not configured."},
                          {"recoverable", true},
                          {"recommended_action", "Configure the VPN server address in Settings"}};
  if (cfg.username.empty())
    return nlohmann::json{{"ok", false}, {"error_type", "config_invalid"},
                          {"message", "VPN username is not configured."},
                          {"recoverable", true},
                          {"recommended_action", "Configure your VPN username in Settings"}};
  if (plaintext_password.empty() && cfg.password.empty())
    return nlohmann::json{{"ok", false}, {"error_type", "config_invalid"},
                          {"message", "VPN password is not configured."},
                          {"recoverable", true},
                          {"recommended_action", "Configure your VPN password in Settings"}};
  if (!tunnel::write_script(cfg)) {
    return nlohmann::json{{"ok", false}, {"error_type", "native_failure"},
                          {"message", "Failed to generate tunnel script."},
                          {"recoverable", true},
                          {"recommended_action", "Retry the operation"}};
  }

#ifdef _WIN32
  nlohmann::json drivers = nlohmann::json::object();
  // Basic driver validation for Windows
  std::string wintun_path = utils::get_bundled_wintun_path();
  if (cfg.windows_tunnel_driver == "wintun" && wintun_path.empty()) {
    return nlohmann::json{{"ok", false}, {"error_type", "runtime_missing"},
                          {"message", "Wintun is selected but bundled wintun.dll is missing."},
                          {"recoverable", true},
                          {"recommended_action", "Rebuild the desktop package with the bundled native runtime assets"}};
  }
#endif

  // Check for already-running VPN
  pid_t existing_pid = read_pid();
  if (existing_pid <= 0 || !is_process_alive(existing_pid))
    existing_pid = find_openconnect_pid();
  if (existing_pid > 0) {
    return nlohmann::json{{"ok", false}, {"error_type", "native_failure"},
                          {"message", "VPN is already running (PID " + std::to_string(existing_pid) + ")."},
                          {"recoverable", true},
                          {"recommended_action", "Disconnect the current VPN session first"}};
  }

  int rc = start_with_password(cfg, plaintext_password, retry_limit);
  if (rc != 0) {
    return nlohmann::json{{"ok", false}, {"error_type", "native_failure"},
                          {"message", "VPN connection failed."},
                          {"recoverable", true},
                          {"recommended_action", "Check VPN logs for details and retry"}};
  }

  // Return the new status after successful start
  return direct_status_json(cfg);
}

} // namespace vpn
} // namespace ecnuvpn
