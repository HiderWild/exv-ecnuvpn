#include "helper.hpp"
#include "helper_ipc.hpp"

#include "logger.hpp"
#include "tunnel.hpp"
#include "utils.hpp"
#include "vpn.hpp"
#include "virtual_network.hpp"

#include <cerrno>
#include <cctype>
#include <csignal>
#include <cstring>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#ifndef _WIN32
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#else
#include <windows.h>
#include <io.h>
#include <thread>
#ifdef _MSC_VER
typedef unsigned int uid_t;
typedef unsigned int gid_t;
typedef int pid_t;
#endif
#endif
#include <vector>


namespace ecnuvpn {
namespace helper {

namespace {

#ifdef __APPLE__
constexpr const char *kHelperLabel = "com.ecnu.exv.helper";
constexpr const char *kHelperPlistPath =
    "/Library/LaunchDaemons/com.ecnu.exv.helper.plist";
constexpr const char *kHelperSocketPath = "/var/run/exv-helper.sock";
constexpr const char *kHelperStatePath = "/var/run/exv-helper-session.json";
#elif defined(__linux__)
constexpr const char *kHelperServiceName = "exv-helper";
constexpr const char *kHelperServicePath =
    "/etc/systemd/system/exv-helper.service";
constexpr const char *kHelperSocketPath = "/var/run/exv-helper.sock";
constexpr const char *kHelperStatePath = "/var/run/exv-helper-session.json";
#elif defined(_WIN32)
constexpr const char *kHelperServiceName = "exv-helper";
constexpr const char *kHelperPipePath = "\\\\.\\pipe\\exv-helper";
constexpr const char *kHelperStatePath = "C:\\ProgramData\\exv-helper-session.json";
#endif
#ifdef _WIN32
constexpr const char *kStableInstallPath = "C:\\Program Files\\ECNU-VPN\\exv.exe";
#else
constexpr const char *kStableInstallPath = "/usr/local/bin/exv";
#endif

volatile sig_atomic_t daemon_stop_requested = 0;

struct SessionState {
  uid_t uid = static_cast<uid_t>(-1);
  gid_t gid = static_cast<gid_t>(-1);
  std::string username;
  std::string home;
  std::string config_dir;
  std::string server;
  int route_count = 0;
  int retry_limit = 0;
};

struct RuntimeSnapshot {
  bool running = false;
  pid_t pid = -1;
  pid_t supervisor_pid = -1;
  bool network_ready = false;
  std::string interface_name;
  uint64_t rx_bytes = 0;
  uint64_t tx_bytes = 0;
  std::chrono::steady_clock::time_point last_traffic_update{};
  std::string internal_ip;
  std::string interfaces_output;
};

struct SemanticVersion {
  int major = -1;
  int minor = -1;
  int patch = -1;
};

std::string pid_path_for(const SessionState &state) {
  return state.config_dir + "/ecnuvpn.pid";
}

std::string supervisor_pid_path_for(const SessionState &state) {
  return state.config_dir + "/ecnuvpn-supervisor.pid";
}

std::string route_ready_path_for(const SessionState &state) {
  return state.config_dir + "/route-ready";
}

void remove_file_if_exists(const std::string &path) {
  if (utils::file_exists(path)) {
    std::remove(path.c_str());
  }
}

bool prompt_confirm(const std::string &question, bool default_yes) {
  // When stdin is not a TTY (e.g. osascript elevation), auto-confirm.
  // The user already authorized via the macOS admin dialog.
  #ifdef _WIN32
  if (!_isatty(_fileno(stdin)))
  #else
  if (!isatty(STDIN_FILENO))
  #endif
    return true;
  std::cout << "  " << question << (default_yes ? " [Y/n]: " : " [y/N]: ");
  std::string input;
  std::getline(std::cin, input);
  input = utils::trim(input);
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
  if (!version || !utils::file_exists(path))
    return false;

  std::string output = utils::trim(
      utils::run_command_output(utils::shell_quote(path) + " version 2>/dev/null"));
  if (output.empty())
    return false;
  return parse_semantic_version(output, version);
}

bool uninstall_existing_stable_exv() {
  if (!utils::file_exists(kStableInstallPath))
    return true;

  utils::print_info(
      "Running service uninstall using the existing stable exv before replacement...");
  if (utils::run_command(utils::shell_quote(kStableInstallPath) +
                         " service uninstall") != 0) {
    utils::print_error(
        "Failed to uninstall the existing helper service before replacing /usr/local/bin/exv.");
    return false;
  }
  return true;
}

bool copy_file_contents(const std::string &source_path,
                        const std::string &target_path,
                        int *error_number = nullptr) {
#ifndef _WIN32
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
#else
  (void)source_path;
  (void)target_path;
  if (error_number)
    *error_number = 0;
  return false;
#endif
}

int copy_self_to_stable_path_and_reexec(const std::string &current_path) {
#ifndef _WIN32
  SemanticVersion current_version;
  bool current_version_ok = parse_semantic_version(ECNUVPN_VERSION, &current_version);
  bool stable_exists = utils::file_exists(kStableInstallPath);

  utils::print_warning(
      "EXV helper service should be installed from a stable system path.");
  std::cout << utils::DIM << "  Current executable: " << current_path
            << utils::RESET << std::endl;
  std::cout << utils::DIM << "  Stable target: " << kStableInstallPath
            << utils::RESET << std::endl;
  if (current_version_ok) {
    std::cout << utils::DIM << "  Current version: "
              << format_semantic_version(current_version) << utils::RESET
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
    bool stable_version_ok = read_binary_version(kStableInstallPath, &stable_version);
    if (stable_version_ok) {
      std::cout << utils::DIM << "  Existing stable version: "
                << format_semantic_version(stable_version) << utils::RESET
                << std::endl;
    } else {
      std::cout << utils::DIM << "  Existing stable version: unknown"
                << utils::RESET << std::endl;
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
    utils::print_info("Service installation canceled.");
    return 1;
  }

  if (stable_exists && !uninstall_existing_stable_exv()) {
    return 1;
  }

  if (!utils::ensure_dir("/usr/local") || !utils::ensure_dir("/usr/local/bin")) {
    utils::print_error("Failed to ensure /usr/local/bin exists.");
    return 1;
  }

  utils::print_info("Copying current exv binary to /usr/local/bin/exv ...");
  int copy_error = 0;
  if (!copy_file_contents(current_path, kStableInstallPath, &copy_error)) {
    utils::print_error("Failed to copy exv to /usr/local/bin/exv: " +
                       std::string(std::strerror(copy_error)));
    return 1;
  }

  utils::print_success("Stable exv binary updated at /usr/local/bin/exv.");
  utils::print_info("Re-running service installation from the copied binary...");
  execl(kStableInstallPath, kStableInstallPath, "service", "install",
        static_cast<char *>(nullptr));

  utils::print_error("Failed to launch /usr/local/bin/exv: " +
                     std::string(std::strerror(errno)));
  return 1;
#else
  (void)current_path;
  return 1;
#endif
}

void clear_runtime_state(const SessionState &state) {
  if (state.config_dir.empty())
    return;
  remove_file_if_exists(pid_path_for(state));
  remove_file_if_exists(supervisor_pid_path_for(state));
  remove_file_if_exists(route_ready_path_for(state));
}

pid_t read_pid_file(const std::string &path) {
  if (!utils::file_exists(path))
    return -1;
  std::string content = utils::trim(utils::read_file(path));
  if (content.empty())
    return -1;
  try {
    return static_cast<pid_t>(std::stoi(content));
  } catch (...) {
    return -1;
  }
}

bool is_process_alive(pid_t pid) {
  if (pid <= 0)
    return false;
#ifndef _WIN32
  if (kill(pid, 0) == 0)
    return true;
  return errno == EPERM;
#else
  HANDLE hProcess = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, static_cast<DWORD>(pid));
  if (!hProcess)
    return false;
  DWORD exitCode = 0;
  BOOL ok = GetExitCodeProcess(hProcess, &exitCode);
  CloseHandle(hProcess);
  return ok && exitCode == STILL_ACTIVE;
#endif
}

pid_t find_openconnect_pid() {
#ifndef _WIN32
  std::string output = utils::trim(utils::run_command_output("pgrep -x openconnect"));
  if (output.empty())
    return -1;
  std::istringstream iss(output);
  std::string first;
  std::getline(iss, first);
  try {
    return static_cast<pid_t>(std::stoi(first));
  } catch (...) {
    return -1;
  }
#else
  // Use tasklist to find openconnect.exe PID
  std::string output = utils::run_command_output("tasklist /FI \"IMAGENAME eq openconnect.exe\" /NH /FO CSV 2>nul");
  // Parse CSV: "openconnect.exe","1234","Console","1","4,096 K"
  auto pos = output.find(',');
  if (pos == std::string::npos || pos < 2) return -1;
  // Extract PID between first and second comma - find the second field
  auto start = output.find('"', pos + 1);
  if (start == std::string::npos) return -1;
  auto end = output.find('"', start + 1);
  if (end == std::string::npos) return -1;
  std::string pid_str = output.substr(start + 1, end - start - 1);
  try {
    return static_cast<pid_t>(std::stoi(pid_str));
  } catch (...) {
    return -1;
  }
#endif
}

static void kill_all_supervisors() {
#ifndef _WIN32
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
#endif
}

bool read_route_ready(const SessionState &state, std::string *interface_name,
                      std::string *internal_ip) {
  std::string path = route_ready_path_for(state);
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

nlohmann::json to_json(const SessionState &state) {
  return nlohmann::json{{"uid", static_cast<unsigned int>(state.uid)},
                        {"gid", static_cast<unsigned int>(state.gid)},
                        {"username", state.username},
                        {"home", state.home},
                        {"config_dir", state.config_dir},
                        {"server", state.server},
                        {"route_count", state.route_count},
                        {"retry_limit", state.retry_limit}};
}

bool from_json(const nlohmann::json &j, SessionState *state) {
  if (!state)
    return false;
  try {
    state->uid = static_cast<uid_t>(j.at("uid").get<unsigned int>());
    state->gid = static_cast<gid_t>(j.at("gid").get<unsigned int>());
    state->username = j.at("username").get<std::string>();
    state->home = j.at("home").get<std::string>();
    state->config_dir = j.at("config_dir").get<std::string>();
    state->server = j.value("server", std::string());
    state->route_count = j.value("route_count", 0);
    state->retry_limit = j.value("retry_limit", 0);
    return true;
  } catch (...) {
    return false;
  }
}

bool save_session_state(const SessionState &state) {
  std::ofstream ofs(kHelperStatePath);
  if (!ofs.is_open())
    return false;
  ofs << to_json(state).dump(2);
  ofs.close();
#ifndef _WIN32
  chmod(kHelperStatePath, 0600);
#endif
  return ofs.good();
}

bool load_session_state(SessionState *state) {
  if (!state || !utils::file_exists(kHelperStatePath))
    return false;
  try {
    nlohmann::json j = nlohmann::json::parse(utils::read_file(kHelperStatePath));
    return from_json(j, state);
  } catch (...) {
    return false;
  }
}

void clear_session_state() { remove_file_if_exists(kHelperStatePath); }

RuntimeSnapshot inspect_runtime(const SessionState &state) {
  RuntimeSnapshot snapshot;
  if (state.config_dir.empty())
    return snapshot;

  snapshot.supervisor_pid = read_pid_file(supervisor_pid_path_for(state));
  if (!is_process_alive(snapshot.supervisor_pid))
    snapshot.supervisor_pid = -1;

  snapshot.pid = read_pid_file(pid_path_for(state));
  if (!is_process_alive(snapshot.pid))
    snapshot.pid = -1;

  if (snapshot.pid <= 0)
    snapshot.pid = find_openconnect_pid();

  snapshot.network_ready =
      read_route_ready(state, &snapshot.interface_name, &snapshot.internal_ip);
  snapshot.running = snapshot.pid > 0 || snapshot.supervisor_pid > 0;

  if (snapshot.running) {
#ifdef __APPLE__
    snapshot.interfaces_output =
        utils::run_command_output("ifconfig | grep -A 2 'utun' | head -20");
#elif defined(_WIN32)
    snapshot.interfaces_output =
        utils::run_command_output("netsh interface show interface 2>nul");
#else
    snapshot.interfaces_output =
        utils::run_command_output("ip addr show type tun 2>/dev/null | head -20");
#endif
  }

  return snapshot;
}

bool create_request_file(const nlohmann::json &request, std::string *out_path) {
  if (!out_path)
    return false;

#ifdef _WIN32
  char temp_path[MAX_PATH];
  if (GetTempPathA(MAX_PATH, temp_path) == 0)
    return false;

  char temp_file[MAX_PATH];
  if (GetTempFileNameA(temp_path, "exv", 0, temp_file) == 0)
    return false;

  std::string payload = request.dump();
  HANDLE hFile = CreateFileA(temp_file, GENERIC_WRITE, 0, NULL,
                             CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
  if (hFile == INVALID_HANDLE_VALUE) {
    DeleteFileA(temp_file);
    return false;
  }

  DWORD written = 0;
  BOOL ok = WriteFile(hFile, payload.data(), static_cast<DWORD>(payload.size()),
                      &written, NULL);
  CloseHandle(hFile);

  if (!ok || written != payload.size()) {
    DeleteFileA(temp_file);
    return false;
  }

  *out_path = temp_file;
  return true;
#else
  char path_template[] = "/var/run/exv-helper-request-XXXXXX";
  int fd = mkstemp(path_template);
  if (fd < 0)
    return false;

  chmod(path_template, 0600);
  std::string payload = request.dump();
  ssize_t written = write(fd, payload.data(), payload.size());
  close(fd);
  if (written != static_cast<ssize_t>(payload.size())) {
    remove_file_if_exists(path_template);
    return false;
  }

  *out_path = path_template;
  return true;
#endif
}

void daemon_signal_handler(int) {
  daemon_stop_requested = 1;
}

void set_close_on_exec(int fd) {
#ifndef _WIN32
  if (fd < 0)
    return;

  int flags = fcntl(fd, F_GETFD);
  if (flags < 0)
    return;
  fcntl(fd, F_SETFD, flags | FD_CLOEXEC);
#else
  (void)fd;
#endif
}

#ifndef _WIN32
bool connect_socket(int *out_fd) {
  if (!out_fd)
    return false;

  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0)
    return false;

  sockaddr_un addr {};
  addr.sun_family = AF_UNIX;
  std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", kHelperSocketPath);

  if (connect(fd, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) != 0) {
    close(fd);
    return false;
  }

  *out_fd = fd;
  return true;
}
#endif

bool send_request(const nlohmann::json &request, nlohmann::json *response,
                  std::string *error_message = nullptr,
                  int timeout_seconds = 15) {
  std::string raw;

#ifdef _WIN32
  // Use WaitNamedPipeA with a short timeout so we don't block when the
  // pipe doesn't exist yet (e.g. during service startup polling).
  if (!WaitNamedPipeA("\\\\.\\pipe\\exv-helper", 2000 /* ms */)) {
    if (error_message)
      *error_message = "EXV helper is not available.";
    return false;
  }

  HANDLE hPipe = CreateFileA("\\\\.\\pipe\\exv-helper",
                             GENERIC_READ | GENERIC_WRITE, 0, NULL,
                             OPEN_EXISTING, 0, NULL);
  if (hPipe == INVALID_HANDLE_VALUE) {
    if (error_message)
      *error_message = "EXV helper is not available.";
    return false;
  }

  // Set read timeout on the pipe so we don't hang if the daemon stalls.
  COMMTIMEOUTS timeouts = {};
  timeouts.ReadIntervalTimeout = MAXDWORD;
  timeouts.ReadTotalTimeoutMultiplier = MAXDWORD;
  timeouts.ReadTotalTimeoutConstant = timeout_seconds * 1000;
  SetCommTimeouts(hPipe, &timeouts);

  std::string payload = request.dump();
  payload.push_back('\n');
  DWORD bytesWritten = 0;
  if (!WriteFile(hPipe, payload.c_str(), static_cast<DWORD>(payload.size()),
                 &bytesWritten, NULL) ||
      bytesWritten != payload.size()) {
    if (error_message)
      *error_message = "Failed to send request to EXV helper.";
    CloseHandle(hPipe);
    return false;
  }

  char buffer[1024];
  DWORD bytesRead = 0;
  while (ReadFile(hPipe, buffer, sizeof(buffer), &bytesRead, NULL) && bytesRead > 0) {
    raw.append(buffer, bytesRead);
    if (raw.find('\n') != std::string::npos)
      break;
  }
  CloseHandle(hPipe);
#else
  int fd = -1;
  if (!connect_socket(&fd)) {
    if (error_message)
      *error_message = "EXV helper is not available.";
    return false;
  }

  std::string payload = request.dump();
  payload.push_back('\n');
  if (write(fd, payload.data(), payload.size()) != static_cast<ssize_t>(payload.size())) {
    if (error_message)
      *error_message = "Failed to send request to EXV helper.";
    close(fd);
    return false;
  }
  shutdown(fd, SHUT_WR);

  char buffer[1024];
  ssize_t n = 0;

  struct timeval tv;
  tv.tv_sec = timeout_seconds;
  tv.tv_usec = 0;

  fd_set readfds;
  FD_ZERO(&readfds);
  FD_SET(fd, &readfds);

  int sel_ret = select(fd + 1, &readfds, nullptr, nullptr, &tv);
  if (sel_ret <= 0) {
    if (error_message) {
      if (sel_ret == 0)
        *error_message = "EXV helper request timed out.";
      else
        *error_message = "EXV helper select error.";
    }
    close(fd);
    return false;
  }

  while ((n = read(fd, buffer, sizeof(buffer))) > 0) {
    raw.append(buffer, buffer + n);
    if (raw.find('\n') != std::string::npos)
      break;
  }
  close(fd);
#endif

  std::size_t newline_pos = raw.find('\n');
  if (newline_pos != std::string::npos)
    raw.resize(newline_pos);
  raw = utils::trim(raw);

  if (raw.empty()) {
    if (error_message)
      *error_message = "EXV helper returned an empty response.";
    return false;
  }

  try {
    if (response)
      *response = nlohmann::json::parse(raw);
    return true;
  } catch (...) {
    if (error_message)
      *error_message = "Failed to parse EXV helper response.";
    return false;
  }
}

bool wait_until_available(int attempts = 1, useconds_t delay_us = 0) {
  for (int i = 0; i < attempts; ++i) {
    nlohmann::json response;
    std::string error_message;
    if (send_request(nlohmann::json{{"action", "status"}}, &response,
                     &error_message)) {
      return true;
    }
    if (i + 1 < attempts && delay_us > 0) {
#ifndef _WIN32
      usleep(delay_us);
#else
      Sleep(delay_us / 1000);
#endif
    }
  }
  return false;
}

void reap_finished_request_handlers() {
#ifndef _WIN32
  int status = 0;
  while (waitpid(-1, &status, WNOHANG) > 0) {
  }
#endif
}

nlohmann::json make_error(const std::string &message) {
  return nlohmann::json{{"ok", false}, {"message", message}};
}

nlohmann::json make_status_response(const SessionState &state,
                                    const RuntimeSnapshot &snapshot,
                                    bool ok = true,
                                    const std::string &message = "") {
  nlohmann::json response{{"ok", ok},
                          {"message", message},
                          {"running", snapshot.running},
                          {"pid", snapshot.pid},
                          {"supervisor_pid", snapshot.supervisor_pid},
                          {"network_ready", snapshot.network_ready},
                          {"interface", snapshot.interface_name},
                          {"internal_ip", snapshot.internal_ip},
                          {"rx_bytes", snapshot.rx_bytes},
                          {"tx_bytes", snapshot.tx_bytes},
                          {"server", state.server},
                          {"route_count", state.route_count},
                          {"retry_limit", state.retry_limit},
                          {"owner_username", state.username},
                          {"interfaces_output", snapshot.interfaces_output}};
  virtual_network::add_status_fields(response, snapshot.interface_name);
  return response;
}

bool ensure_same_owner(const SessionState &state, uid_t peer_uid) {
  // Any local user who can reach the helper socket may manage the VPN session.
  // Any local user who can reach the helper socket may manage the VPN session.
  // Socket is mode 0660, group staff (gid 20) — access controlled at OS level.
  (void)state;
  (void)peer_uid;
  return true;
}

nlohmann::json handle_status(uid_t peer_uid) {
  SessionState state;
  if (!load_session_state(&state)) {
    return nlohmann::json{{"ok", true}, {"running", false}};
  }

  if (!ensure_same_owner(state, peer_uid)) {
    return make_error("VPN session belongs to another local user.");
  }

  RuntimeSnapshot snapshot = inspect_runtime(state);
  if (!snapshot.running) {
#ifndef _WIN32
    tunnel::cleanup_routes();
#endif
    clear_runtime_state(state);
    clear_session_state();
    return nlohmann::json{{"ok", true}, {"running", false}};
  }

  return make_status_response(state, snapshot);
}

bool stop_managed_session(const SessionState &state, std::string *message) {
  pid_t supervisor_pid = read_pid_file(supervisor_pid_path_for(state));
  if (!is_process_alive(supervisor_pid))
    supervisor_pid = -1;

  pid_t pid = read_pid_file(pid_path_for(state));
  if (!is_process_alive(pid))
    pid = -1;

  if (pid <= 0)
    pid = find_openconnect_pid();

  if (pid <= 0 && supervisor_pid <= 0) {
    tunnel::cleanup_routes();
    kill_all_supervisors();
    clear_runtime_state(state);
    if (message)
      *message = "No openconnect process found. VPN is not running.";
    return false;
  }

  // Clean up routes before killing openconnect — while the tunnel
  // interface is still valid, route deletion is more reliable.
#ifndef _WIN32
  tunnel::cleanup_routes();
#endif

  if (supervisor_pid > 0)
#ifndef _WIN32
    kill(supervisor_pid, SIGTERM);
#else
    { HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE, static_cast<DWORD>(supervisor_pid)); if (h) { TerminateProcess(h, 1); CloseHandle(h); } }
#endif
  if (pid > 0)
#ifndef _WIN32
    kill(pid, SIGTERM);
#else
    { HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE, static_cast<DWORD>(pid)); if (h) { TerminateProcess(h, 1); CloseHandle(h); } }
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

  if (pid > 0 && is_process_alive(pid))
#ifndef _WIN32
    kill(pid, SIGKILL);
#else
    { HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE, static_cast<DWORD>(pid)); if (h) { TerminateProcess(h, 1); CloseHandle(h); } }
#endif
  if (supervisor_pid > 0 && is_process_alive(supervisor_pid))
#ifndef _WIN32
    kill(supervisor_pid, SIGKILL);
#else
    { HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE, static_cast<DWORD>(supervisor_pid)); if (h) { TerminateProcess(h, 1); CloseHandle(h); } }
#endif

#ifndef _WIN32
  usleep(500000);
#else
  Sleep(500);
#endif

  pid_t remaining_pid = find_openconnect_pid();
  if (remaining_pid > 0 && remaining_pid != pid) {
#ifndef _WIN32
    kill(remaining_pid, SIGKILL);
    usleep(500000);
#else
    { HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE, static_cast<DWORD>(remaining_pid)); if (h) { TerminateProcess(h, 1); CloseHandle(h); } }
    Sleep(500);
#endif
  }

  if ((pid > 0 && is_process_alive(pid)) ||
      (remaining_pid > 0 && is_process_alive(remaining_pid)) ||
      (supervisor_pid > 0 && is_process_alive(supervisor_pid))) {
    if (message)
      *message = "Failed to stop VPN connection!";
    return false;
  }

  #ifndef _WIN32
  kill_all_supervisors();
#endif

  clear_runtime_state(state);
  if (message)
    *message = "VPN connection stopped successfully! 🎉";
  return true;
}

nlohmann::json handle_stop(uid_t peer_uid) {
  SessionState state;
  if (!load_session_state(&state)) {
    // No session state — still clean up any orphaned routes/supervisors
#ifndef _WIN32
    tunnel::cleanup_routes();
    kill_all_supervisors();
#endif
    return make_error("No openconnect process found. VPN is not running.");
  }

  if (!ensure_same_owner(state, peer_uid)) {
    return make_error("VPN session belongs to another local user.");
  }

  std::string message;
  bool ok = stop_managed_session(state, &message);
  if (ok) {
    clear_session_state();
    return nlohmann::json{{"ok", true}, {"message", message}};
  }

  // stop_managed_session failed — belt-and-suspenders cleanup
#ifndef _WIN32
  tunnel::cleanup_routes();
  kill_all_supervisors();
#endif
  RuntimeSnapshot snapshot = inspect_runtime(state);
  if (!snapshot.running)
    clear_session_state();
  return make_error(message);
}

nlohmann::json handle_start(uid_t peer_uid, gid_t peer_gid,
                            const nlohmann::json &request) {
  SessionState existing;
  if (load_session_state(&existing)) {
    RuntimeSnapshot current = inspect_runtime(existing);
    if (current.running) {
      if (!ensure_same_owner(existing, peer_uid)) {
        return make_error("VPN session belongs to another local user.");
      }
      return nlohmann::json{{"ok", false},
                            {"message", "VPN is already running."},
                            {"running", true},
                            {"pid", current.pid},
                            {"supervisor_pid", current.supervisor_pid}};
    }
    clear_runtime_state(existing);
    clear_session_state();
  }

  Config cfg;
  std::string plaintext_password;
  int retry_limit = 0;
  std::string requested_home;
  std::string requested_config_dir;
  try {
    cfg = request.at("config").get<Config>();
    plaintext_password = request.at("password").get<std::string>();
    retry_limit = request.value("retry_limit", 0);
    requested_home = request.value("home", std::string());
    requested_config_dir = request.value("config_dir", std::string());
  } catch (...) {
    return make_error("Invalid start request payload.");
  }

  SessionState state;
  state.uid = peer_uid;
  state.gid = peer_gid;
  state.username = utils::get_username_for_uid(peer_uid);
  state.home = requested_home.empty() ? utils::get_home_for_uid(peer_uid)
                                    : requested_home;
  state.config_dir =
      requested_config_dir.empty() ? utils::get_config_dir_for_uid(peer_uid)
                                   : requested_config_dir;
  state.server = cfg.server;
  state.route_count = static_cast<int>(cfg.routes.size());
  state.retry_limit = retry_limit;

  nlohmann::json worker_request{{"uid", static_cast<unsigned int>(state.uid)},
                                {"gid", static_cast<unsigned int>(state.gid)},
                                {"home", state.home},
                                {"config_dir", state.config_dir},
                                {"retry_limit", retry_limit},
                                {"password", plaintext_password},
                                {"config", cfg}};

  std::string request_path;
  if (!create_request_file(worker_request, &request_path)) {
    return make_error("Failed to prepare helper request file.");
  }

  std::string executable_path = utils::get_executable_path();
  int status = 0;
#ifdef _WIN32
  // Windows: CreateProcess to launch worker
  std::string cmdline =
      "\"" + executable_path + "\" __helper-exec \"" + request_path + "\"";
  std::vector<char> mutable_cmd(cmdline.begin(), cmdline.end());
  mutable_cmd.push_back('\0');
  STARTUPINFOA si = {};
  si.cb = sizeof(si);
  PROCESS_INFORMATION pi = {};
  if (!CreateProcessA(executable_path.c_str(), mutable_cmd.data(), NULL, NULL, TRUE,
                       CREATE_NO_WINDOW, NULL, NULL, &si, &pi)) {
    remove_file_if_exists(request_path);
    return make_error("Failed to launch EXV helper worker.");
  }
  CloseHandle(pi.hThread);
  WaitForSingleObject(pi.hProcess, INFINITE);
  DWORD exitCode = 1;
  GetExitCodeProcess(pi.hProcess, &exitCode);
  CloseHandle(pi.hProcess);
  status = (exitCode == 0) ? 0 : 1;
#else
  pid_t worker_pid = fork();
  if (worker_pid < 0) {
    remove_file_if_exists(request_path);
    return make_error("Failed to launch EXV helper worker.");
  }

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

  status = 0;
  while (waitpid(worker_pid, &status, 0) < 0) {
    if (errno != EINTR) {
      status = -1;
      break;
    }
  }
#endif

  remove_file_if_exists(request_path);

  RuntimeSnapshot snapshot = inspect_runtime(state);
#ifndef _WIN32
  if (WIFEXITED(status) && WEXITSTATUS(status) == 0 && snapshot.running) {
#else
  if (status == 0 && snapshot.running) {
#endif
    save_session_state(state);
    if (snapshot.network_ready) {
      return make_status_response(state, snapshot, true,
                                  "VPN connected successfully!");
    }
    return make_status_response(
        state, snapshot, true,
        "VPN process started, but network routes are not ready yet.");
  }

  clear_runtime_state(state);
  clear_session_state();
  return make_error("Failed to establish the VPN connection. Check logs with: exv logs");
}

nlohmann::json handle_request(uid_t peer_uid, gid_t peer_gid,
                              const nlohmann::json &request) {
  std::string action = request.value("action", std::string());
  if (action == "start")
    return handle_start(peer_uid, peer_gid, request);
  if (action == "stop")
    return handle_stop(peer_uid);
  if (action == "status")
    return handle_status(peer_uid);
  return make_error("Unknown helper action.");
}

bool print_running_status(const nlohmann::json &response) {
  bool running = response.value("running", false);
  if (!running) {
    std::cout << utils::RED << utils::BOLD << "  ● VPN is NOT RUNNING"
              << utils::RESET << std::endl;
    std::cout << std::endl;
    return true;
  }

  std::cout << utils::GREEN << utils::BOLD << "  ● VPN is RUNNING"
            << utils::RESET << std::endl;
  std::cout << std::endl;

  int pid = response.value("pid", -1);
  int supervisor_pid = response.value("supervisor_pid", -1);
  if (pid > 0)
    std::cout << "  PID            : " << pid << std::endl;
  if (supervisor_pid > 0)
    std::cout << "  Supervisor PID : " << supervisor_pid << std::endl;
  std::cout << "  Network Ready  : "
            << (response.value("network_ready", false)
                    ? "yes"
                    : "no (waiting for tunnel script)")
            << std::endl;
  if (response.value("network_ready", false)) {
    std::cout << "  Interface      : "
              << response.value("interface", std::string()) << std::endl;
    std::cout << "  Internal IP    : "
              << response.value("internal_ip", std::string()) << std::endl;
  }
  if (response.value("upstream_virtual_detected", false)) {
    std::cout << "  Route Policy   : EXV campus routes first, upstream virtual adapter preserved"
              << std::endl;
    std::string message =
        response.value("upstream_virtual_message", std::string());
    if (!message.empty())
      std::cout << "  Notice         : " << message << std::endl;
  }

  std::string interfaces_output = response.value("interfaces_output", std::string());
  if (!interfaces_output.empty()) {
    std::cout << std::endl;
    std::cout << utils::DIM << "  Network Interfaces:" << utils::RESET
              << std::endl;
    std::istringstream iss(interfaces_output);
    std::string line;
    while (std::getline(iss, line)) {
      std::cout << "    " << line << std::endl;
    }
  }

  std::cout << std::endl;
  return true;
}

} // namespace

void request_daemon_stop() {
  daemon_stop_requested = 1;

#ifdef _WIN32
  if (WaitNamedPipeA(kHelperPipePath, 200)) {
    HANDLE hPipe = CreateFileA(kHelperPipePath, GENERIC_READ | GENERIC_WRITE,
                               0, NULL, OPEN_EXISTING, 0, NULL);
    if (hPipe != INVALID_HANDLE_VALUE) {
      CloseHandle(hPipe);
    }
  }
#endif
}

bool is_available() {
  return wait_until_available();
}

bool start_via_helper(const Config &cfg, const std::string &plaintext_password,
                      int retry_limit) {
  nlohmann::json response;
  std::string error_message;
  if (!send_request(nlohmann::json{{"action", "start"},
                                   {"config", cfg},
                                   {"password", plaintext_password},
                                   {"retry_limit", retry_limit},
                                   {"home", utils::get_effective_home()},
                                   {"config_dir", utils::get_config_dir()}},
                    &response, &error_message, 120)) {
    utils::print_error(error_message);
    return false;
  }

  if (!response.value("ok", false)) {
    std::string message = response.value("message", std::string("Start failed."));
    if (message.find("already running") != std::string::npos) {
      utils::print_warning(message);
      int pid = response.value("pid", -1);
      int supervisor_pid = response.value("supervisor_pid", -1);
      if (pid > 0 || supervisor_pid > 0) {
        utils::print_info("Use 'exv stop' to stop the current connection first.");
      }
    } else {
      utils::print_error(message);
    }
    return false;
  }

  bool network_ready = response.value("network_ready", false);
  std::cout << std::endl;
  if (network_ready) {
    utils::print_success("VPN connected successfully!");
  } else {
    utils::print_warning("VPN process started, but network routes are not ready yet.");
  }
  int pid = response.value("pid", -1);
  int supervisor_pid = response.value("supervisor_pid", -1);
  if (pid > 0)
    std::cout << utils::DIM << "  PID: " << pid << utils::RESET << std::endl;
  if (supervisor_pid > 0)
    std::cout << utils::DIM << "  Supervisor PID: " << supervisor_pid
              << utils::RESET << std::endl;
  if (network_ready) {
    std::cout << utils::DIM << "  Interface: "
              << response.value("interface", std::string()) << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Internal IP: "
              << response.value("internal_ip", std::string()) << utils::RESET
              << std::endl;
  }
  std::cout << utils::DIM << "  Server: "
            << response.value("server", std::string()) << utils::RESET
            << std::endl;
  std::cout << utils::DIM << "  Routes: " << response.value("route_count", 0)
            << " configured" << utils::RESET << std::endl;
  if (response.value("upstream_virtual_detected", false)) {
    std::string message =
        response.value("upstream_virtual_message", std::string());
    if (!message.empty())
      utils::print_warning(message);
  }
  int helper_retry_limit = response.value("retry_limit", 0);
  if (helper_retry_limit != 0) {
    std::cout << utils::DIM << "  Auto-reconnect: "
              << (helper_retry_limit < 0
                      ? std::string("infinite")
                      : std::to_string(helper_retry_limit) + " retries")
              << utils::RESET << std::endl;
  }
  std::cout << std::endl;
  if (!network_ready) {
    utils::print_info("Check status with: exv status");
    utils::print_info("Check logs with: exv logs");
  }
  utils::print_info("Stop with: exv stop");
  return true;
}

bool stop_via_helper() {
  nlohmann::json response;
  std::string error_message;
  if (!send_request(nlohmann::json{{"action", "stop"}}, &response,
                    &error_message)) {
    utils::print_error(error_message);
    return false;
  }

  if (!response.value("ok", false)) {
    utils::print_error(response.value("message",
                                      std::string("Failed to stop VPN connection.")));
    return false;
  }

  std::cout << std::endl;
  utils::print_success(
      response.value("message", std::string("VPN connection stopped successfully! 🎉")));
  return true;
}

bool show_status_via_helper() {
  nlohmann::json response;
  std::string error_message;
  if (!send_request(nlohmann::json{{"action", "status"}}, &response,
                    &error_message)) {
    utils::print_error(error_message);
    return false;
  }

  if (!response.value("ok", false)) {
    utils::print_error(response.value("message",
                                      std::string("Failed to query EXV helper status.")));
    return false;
  }

  return print_running_status(response);
}

int install_service(const std::string &executable_path) {
#ifdef __APPLE__
  if (!utils::check_root()) {
    utils::print_error("Root privileges required. Please run with sudo.");
    return 1;
  }

  std::string exec_path = executable_path.empty() ? utils::get_executable_path()
                                                  : executable_path;
  if (exec_path.empty()) {
    utils::print_error("Failed to resolve the exv executable path.");
    return 1;
  }

  if (exec_path != kStableInstallPath) {
    return copy_self_to_stable_path_and_reexec(exec_path);
  }

  std::string shell_command =
      "if [ ! -x " + utils::shell_quote(exec_path) +
      " ]; then exit 0; fi; exec " + utils::shell_quote(exec_path) +
      " __helper-daemon";

  std::ostringstream plist;
  plist << "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
  plist << "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" "
           "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";
  plist << "<plist version=\"1.0\">\n";
  plist << "<dict>\n";
  plist << "  <key>Label</key>\n";
  plist << "  <string>" << kHelperLabel << "</string>\n";
  plist << "  <key>ProgramArguments</key>\n";
  plist << "  <array>\n";
  plist << "    <string>/bin/sh</string>\n";
  plist << "    <string>-c</string>\n";
  plist << "    <string>" << shell_command << "</string>\n";
  plist << "  </array>\n";
  plist << "  <key>RunAtLoad</key>\n";
  plist << "  <true/>\n";
  plist << "  <key>KeepAlive</key>\n";
  plist << "  <dict>\n";
  plist << "    <key>SuccessfulExit</key>\n";
  plist << "    <false/>\n";
  plist << "  </dict>\n";
  plist << "</dict>\n";
  plist << "</plist>\n";

  std::ofstream ofs(kHelperPlistPath);
  if (!ofs.is_open()) {
    utils::print_error("Failed to write LaunchDaemon plist: " +
                       std::string(kHelperPlistPath));
    return 1;
  }
  ofs << plist.str();
  ofs.close();
  chmod(kHelperPlistPath, 0644);

  utils::run_command(std::string("launchctl bootout system ") + kHelperPlistPath +
                     " >/dev/null 2>&1");
  if (utils::run_command(std::string("launchctl bootstrap system ") +
                         kHelperPlistPath) != 0) {
    utils::print_error("Failed to bootstrap EXV helper LaunchDaemon.");
    return 1;
  }

  bool helper_ready = wait_until_available(50, 100000);

  // Fix config directory ownership: the install process ran as root and may
  // have created/modified ~/.ecnuvpn/ with root ownership. Restore it to the
  // invoking user so the desktop app can read/write config normally.
  utils::fix_config_dir_ownership();

  utils::print_success("EXV helper service installed.");
  if (!helper_ready) {
    utils::print_warning(
        "Helper service was installed, but it has not responded on the socket yet.");
    utils::print_info("Run 'exv service status' again in a moment if needed.");
  }
  utils::print_info("You can now run 'exv' and 'exv stop' without sudo.");
  return 0;
#elif defined(__linux__)
  // Linux systemd service installation
  if (!utils::check_root()) {
    utils::print_error("Root privileges required. Please run with sudo.");
    return 1;
  }

  std::string exec_path = executable_path.empty() ? utils::get_executable_path()
                                                  : executable_path;
  if (exec_path.empty()) {
    utils::print_error("Failed to resolve the exv executable path.");
    return 1;
  }

  std::ofstream ofs(kHelperServicePath);
  if (!ofs.is_open()) {
    utils::print_error("Failed to write systemd unit file: " +
                       std::string(kHelperServicePath));
    return 1;
  }
  ofs << "[Unit]\n";
  ofs << "Description=ECNU VPN Helper Daemon\n";
  ofs << "After=network.target\n\n";
  ofs << "[Service]\n";
  ofs << "Type=forking\n";
  ofs << "ExecStart=" << exec_path << " __helper-daemon\n";
  ofs << "Restart=on-failure\n";
  ofs << "RestartSec=5\n\n";
  ofs << "[Install]\n";
  ofs << "WantedBy=multi-user.target\n";
  ofs.close();

  std::string reload_cmd = "systemctl daemon-reload";
  if (utils::run_command(reload_cmd) != 0) {
    utils::print_error("Failed to reload systemd daemon.");
    return 1;
  }

  std::string enable_cmd = "systemctl enable " + std::string(kHelperServiceName);
  if (utils::run_command(enable_cmd) != 0) {
    utils::print_error("Failed to enable EXV helper service.");
    return 1;
  }

  std::string start_cmd = "systemctl start " + std::string(kHelperServiceName);
  if (utils::run_command(start_cmd) != 0) {
    utils::print_error("Failed to start EXV helper service.");
    return 1;
  }

  bool helper_ready = wait_until_available(50, 100000);

  utils::print_success("EXV helper service installed.");
  if (!helper_ready) {
    utils::print_warning(
        "Helper service was installed, but it has not responded on the socket yet.");
    utils::print_info("Run 'exv service status' again in a moment if needed.");
  }
  utils::print_info("You can now run 'exv' and 'exv stop' without sudo.");
  return 0;
#elif defined(_WIN32)
  if (!utils::check_root()) {
    utils::print_error("Administrator privileges required. Please run from an elevated prompt.");
    return 1;
  }

  utils::print_info("Opening Service Control Manager...");
  SC_HANDLE hSCM = OpenSCManagerA(NULL, NULL, SC_MANAGER_CREATE_SERVICE);
  if (!hSCM) {
    logger::error("Cannot open Service Control Manager");
    return 1;
  }

  std::string exec_path = executable_path.empty() ? utils::get_executable_path()
                                                  : executable_path;
  if (exec_path.empty()) {
    logger::error("Failed to resolve the exv executable path.");
    CloseServiceHandle(hSCM);
    return 1;
  }

  std::filesystem::path exec_fs_path(exec_path);
  std::filesystem::path helper_path =
      exec_fs_path.parent_path() / "exv-helper.exe";

  std::string binary_path;
  if (std::filesystem::exists(helper_path)) {
    binary_path = "\"" + helper_path.string() + "\" --service";
  } else {
    utils::print_warning(
        "Dedicated exv-helper.exe was not found next to exv.exe. Falling back to legacy in-process helper service mode.");
    binary_path = "\"" + exec_path + "\" __helper-daemon";
  }

  utils::print_info("Registering helper service...");
  SC_HANDLE hService = CreateServiceA(
      hSCM, kHelperServiceName, "ECNU VPN Helper",
      SERVICE_ALL_ACCESS, SERVICE_WIN32_OWN_PROCESS,
      SERVICE_AUTO_START, SERVICE_ERROR_NORMAL,
      binary_path.c_str(), NULL, NULL, NULL, NULL, NULL);

  if (!hService) {
    DWORD err = GetLastError();
    if (err == ERROR_SERVICE_EXISTS) {
      utils::print_info(
          "Helper service is already installed. Refreshing service configuration...");
      hService = OpenServiceA(hSCM, kHelperServiceName,
                              SERVICE_CHANGE_CONFIG | SERVICE_QUERY_STATUS |
                                  SERVICE_START | SERVICE_STOP);
      if (!hService) {
        logger::error("OpenService failed: " +
                      std::to_string(GetLastError()));
        CloseServiceHandle(hSCM);
        return 1;
      }

      if (!ChangeServiceConfigA(hService, SERVICE_NO_CHANGE,
                                SERVICE_NO_CHANGE, SERVICE_NO_CHANGE,
                                binary_path.c_str(), NULL, NULL, NULL, NULL,
                                NULL, NULL)) {
        logger::error("ChangeServiceConfig failed: " +
                      std::to_string(GetLastError()));
        CloseServiceHandle(hService);
        CloseServiceHandle(hSCM);
        return 1;
      }

      SERVICE_STATUS service_status = {};
      if (QueryServiceStatus(hService, &service_status) &&
          service_status.dwCurrentState != SERVICE_STOPPED) {
        utils::print_info("Restarting helper service to apply the new binary path...");
        ControlService(hService, SERVICE_CONTROL_STOP, &service_status);
        for (int i = 0; i < 50; ++i) {
          if (!QueryServiceStatus(hService, &service_status) ||
              service_status.dwCurrentState == SERVICE_STOPPED) {
            break;
          }
          Sleep(100);
        }
      }
    } else {
      logger::error("CreateService failed: " + std::to_string(err));
      CloseServiceHandle(hSCM);
      return 1;
    }
  }

  utils::print_info("Starting helper service...");
  if (!StartService(hService, 0, NULL)) {
    DWORD err = GetLastError();
    if (err != ERROR_SERVICE_ALREADY_RUNNING) {
      logger::error("StartService failed: " + std::to_string(err));
      CloseServiceHandle(hService);
      CloseServiceHandle(hSCM);
      return 1;
    }
  }
  CloseServiceHandle(hService);
  CloseServiceHandle(hSCM);

  utils::print_info("Waiting for helper to become ready...");
  bool helper_ready = wait_until_available(50, 100000);

  utils::print_success("EXV helper service installed.");
  if (!helper_ready) {
    utils::print_warning(
        "Helper service was installed, but it has not responded yet.");
    utils::print_info("Run 'exv service status' again in a moment if needed.");
  }
  utils::print_info("You can now run 'exv' and 'exv stop' without elevation.");
  return 0;
#endif
}

int uninstall_service() {
#ifdef __APPLE__
  if (!utils::check_root()) {
    utils::print_error("Root privileges required. Please run with sudo.");
    return 1;
  }

  nlohmann::json response;
  std::string error_message;
  send_request(nlohmann::json{{"action", "stop"}}, &response, &error_message);

  // Belt-and-suspenders: clean up routes even if helper stop failed
  tunnel::cleanup_routes();
  kill_all_supervisors();

  utils::run_command(std::string("launchctl bootout system ") + kHelperPlistPath +
                     " >/dev/null 2>&1");
  remove_file_if_exists(kHelperPlistPath);
  remove_file_if_exists(kHelperSocketPath);
  clear_session_state();

  utils::print_success("EXV helper service uninstalled.");
  return 0;
#elif defined(__linux__)
  if (!utils::check_root()) {
    utils::print_error("Root privileges required. Please run with sudo.");
    return 1;
  }

  nlohmann::json response;
  std::string error_message;
  send_request(nlohmann::json{{"action", "stop"}}, &response, &error_message);

  std::string stop_cmd = "systemctl stop " + std::string(kHelperServiceName);
  utils::run_command(stop_cmd + " >/dev/null 2>&1");
  std::string disable_cmd = "systemctl disable " + std::string(kHelperServiceName);
  utils::run_command(disable_cmd + " >/dev/null 2>&1");
  remove_file_if_exists(kHelperServicePath);
  remove_file_if_exists(kHelperSocketPath);
  utils::run_command("systemctl daemon-reload >/dev/null 2>&1");
  clear_session_state();

  utils::print_success("EXV helper service uninstalled.");
  return 0;
#elif defined(_WIN32)
  SC_HANDLE hSCM = OpenSCManagerA(NULL, NULL, SC_MANAGER_CONNECT);
  if (!hSCM) return 1;

  SC_HANDLE hService = OpenServiceA(hSCM, kHelperServiceName, SERVICE_STOP | DELETE);
  if (!hService) {
    std::cout << "Helper service is not installed.\n";
    CloseServiceHandle(hSCM);
    return 0;
  }

  SERVICE_STATUS status;
  ControlService(hService, SERVICE_CONTROL_STOP, &status);
  DeleteService(hService);

  CloseServiceHandle(hService);
  CloseServiceHandle(hSCM);

  std::cout << "Helper service uninstalled.\n";
  return 0;
#endif
}

int show_service_status() {
#ifdef __APPLE__
  utils::print_header("EXV Service Status");

  bool plist_exists = utils::file_exists(kHelperPlistPath);
  bool available = plist_exists ? wait_until_available(10, 100000) : false;
  std::cout << "  Installed       : " << (plist_exists ? "yes" : "no")
            << std::endl;
  std::cout << "  Socket Ready    : " << (available ? "yes" : "no")
            << std::endl;

  if (available) {
    nlohmann::json response;
    std::string error_message;
    if (send_request(nlohmann::json{{"action", "status"}}, &response,
                     &error_message) && response.value("ok", false)) {
      std::cout << "  VPN Running     : "
                << (response.value("running", false) ? "yes" : "no")
                << std::endl;
      if (response.value("running", false)) {
        std::cout << "  Session Owner   : "
                  << response.value("owner_username", std::string()) << std::endl;
      }
    }
  }

  std::cout << std::endl;
  return 0;
#elif defined(__linux__)
  utils::print_header("EXV Service Status");

  bool service_exists = utils::file_exists(kHelperServicePath);
  bool available = service_exists ? wait_until_available(10, 100000) : false;
  std::cout << "  Installed       : " << (service_exists ? "yes" : "no")
            << std::endl;
  std::cout << "  Socket Ready    : " << (available ? "yes" : "no")
            << std::endl;

  if (available) {
    nlohmann::json response;
    std::string error_message;
    if (send_request(nlohmann::json{{"action", "status"}}, &response,
                     &error_message) && response.value("ok", false)) {
      std::cout << "  VPN Running     : "
                << (response.value("running", false) ? "yes" : "no")
                << std::endl;
      if (response.value("running", false)) {
        std::cout << "  Session Owner   : "
                  << response.value("owner_username", std::string()) << std::endl;
      }
    }
  }

  std::cout << std::endl;
  return 0;
#elif defined(_WIN32)
  utils::print_header("EXV Service Status");

  SC_HANDLE hSCM = OpenSCManagerA(NULL, NULL, SC_MANAGER_CONNECT);
  if (!hSCM) {
    std::cout << "  Installed       : unknown (cannot open SCM)\n";
    return 1;
  }
  SC_HANDLE hService = OpenServiceA(hSCM, kHelperServiceName, SERVICE_QUERY_STATUS);
  bool installed = (hService != NULL);
  std::cout << "  Installed       : " << (installed ? "yes" : "no") << std::endl;

  bool available = false;
  if (installed) {
    SERVICE_STATUS status;
    QueryServiceStatus(hService, &status);
    std::cout << "  State           : " << (status.dwCurrentState == SERVICE_RUNNING ? "running" : "stopped") << std::endl;
    if (status.dwCurrentState == SERVICE_RUNNING) {
      available = wait_until_available(10, 100000);
    }
    CloseServiceHandle(hService);
  }
  std::cout << "  Socket Ready    : " << (available ? "yes" : "no") << std::endl;

  if (available) {
    nlohmann::json response;
    std::string error_message;
    if (send_request(nlohmann::json{{"action", "status"}}, &response,
                     &error_message) && response.value("ok", false)) {
      std::cout << "  VPN Running     : "
                << (response.value("running", false) ? "yes" : "no")
                << std::endl;
      if (response.value("running", false)) {
        std::cout << "  Session Owner   : "
                  << response.value("owner_username", std::string()) << std::endl;
      }
    }
  }

  std::cout << std::endl;
  CloseServiceHandle(hSCM);
  return 0;
#endif
}

int worker_main(const std::string &request_path) {
  try {
    nlohmann::json request =
        nlohmann::json::parse(utils::read_file(request_path));
    Config cfg = request.at("config").get<Config>();
    std::string plaintext_password = request.at("password").get<std::string>();
    int retry_limit = request.value("retry_limit", 0);
    uid_t uid = static_cast<uid_t>(request.at("uid").get<unsigned int>());
    gid_t gid = static_cast<gid_t>(request.at("gid").get<unsigned int>());
    std::string home = request.at("home").get<std::string>();
    std::string config_dir = request.at("config_dir").get<std::string>();

    utils::set_runtime_path_override(home, config_dir);
    utils::set_runtime_owner(uid, gid);
    logger::init();
    int result = vpn::start_with_password(cfg, plaintext_password, retry_limit);
    utils::clear_runtime_owner();
    utils::clear_runtime_path_override();
    return result;
  } catch (...) {
    return 1;
  }
}

int daemon_main() {
  signal(SIGTERM, daemon_signal_handler);
  signal(SIGINT, daemon_signal_handler);
#ifndef _WIN32
  signal(SIGPIPE, SIG_IGN);
#endif

  auto ipc = create_ipc_server();

#ifdef _WIN32
  constexpr const char *ipc_path = "\\\\.\\pipe\\exv-helper";
#else
  constexpr const char *ipc_path = "/var/run/exv-helper.sock";
  remove_file_if_exists(ipc_path);
#endif

  if (!ipc->start(ipc_path)) {
    return 1;
  }

  while (!daemon_stop_requested) {
#ifndef _WIN32
    reap_finished_request_handlers();
#endif

    if (!ipc->accept_client()) {
      if (daemon_stop_requested)
        break;
      continue;
    }

    if (!ipc->verify_client()) {
      ipc->close_client();
      continue;
    }

    std::string raw = ipc->read_request();
    unsigned int peer_uid = ipc->peer_uid();
    unsigned int peer_gid = ipc->peer_gid();

#ifdef _WIN32
    // Windows: use threads instead of fork (no fork available)
    // Each request is processed in a background thread so long-running
    // operations (e.g. VPN start) don't block new client connections.
    IpcServer *ipc_ptr = ipc.get();
    std::thread([ipc_ptr, raw, peer_uid, peer_gid]() {
      nlohmann::json response;
      try {
        nlohmann::json request = nlohmann::json::parse(raw);
        response = handle_request(peer_uid, peer_gid, request);
      } catch (...) {
        response = make_error("Failed to parse helper request.");
      }
      ipc_ptr->send_response(response.dump());
    }).detach();
#else
    // POSIX: fork a child to handle the request
    pid_t handler_pid = fork();
    if (handler_pid < 0) {
      nlohmann::json response =
          make_error("Failed to launch EXV helper request handler.");
      ipc->send_response(response.dump());
      ipc->close_client();
      continue;
    }

    if (handler_pid == 0) {
      signal(SIGTERM, SIG_DFL);
      signal(SIGINT, SIG_DFL);
      signal(SIGCHLD, SIG_DFL);
      signal(SIGPIPE, SIG_IGN);
      ipc->close_server();
      nlohmann::json response;
      try {
        nlohmann::json request = nlohmann::json::parse(raw);
        response = handle_request(peer_uid, peer_gid, request);
      } catch (...) {
        response = make_error("Failed to parse helper request.");
      }
      bool sent = ipc->send_response(response.dump());
      _exit(0);
    }

    // Wait for the child to finish writing the response before closing
    // the client fd.  The child inherited client_fd_ across fork(); if
    // the parent closes it first, the child's write() fails and the
    // client receives an empty response.
    int status = 0;
    while (waitpid(handler_pid, &status, 0) < 0 && errno == EINTR)
      ;
    ipc->close_client();
#endif
  }

#ifndef _WIN32
  reap_finished_request_handlers();
#endif

  SessionState state;
  if (load_session_state(&state)) {
    std::string message;
    stop_managed_session(state, &message);
    clear_session_state();
  }

  ipc->close();
#ifndef _WIN32
  remove_file_if_exists(ipc_path);
#endif
  return 0;
}

} // namespace helper
} // namespace ecnuvpn
