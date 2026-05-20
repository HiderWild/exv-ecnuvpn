#include "utils.hpp"

#include "platform/common/path_utils.hpp"

#include <array>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif
#ifdef _WIN32
#include <windows.h>
#include <iphlpapi.h>
#include <direct.h>
#include <io.h>
#include <sys/stat.h>
#else
#include <pwd.h>
#include <sys/stat.h>
#include <unistd.h>
#endif
#include <memory>
#include <sstream>
#include <vector>

namespace ecnuvpn {
namespace utils {

static std::string runtime_home_override;
static std::string runtime_config_dir_override;
static uid_t runtime_owner_uid = static_cast<uid_t>(-1);
static gid_t runtime_owner_gid = static_cast<gid_t>(-1);

static std::string expand_home_with_base(const std::string &path,
                                         const std::string &home) {
  if (!path.empty() && path[0] == '~' && !home.empty()) {
    return home + path.substr(1);
  }
  return path;
}

static std::string get_config_dir_for_home(const std::string &home) {
  std::string default_dir = platform::default_config_dir_for_home(home);
  if (default_dir.empty())
    return "";

  std::string redirect = platform::redirect_path_for_home(home);
  if (!redirect.empty()) {
    std::ifstream rf(redirect);
    if (rf.is_open()) {
      std::string dir;
      std::getline(rf, dir);
      dir = trim(dir);
      if (!dir.empty())
        return expand_home_with_base(dir, home);
    }
  }

  return default_dir;
}

static std::vector<std::string> candidate_runtime_dirs() {
  std::vector<std::string> dirs;

  const char *env_runtime_dir = std::getenv("ECNUVPN_RUNTIME_DIR");
  if (env_runtime_dir && *env_runtime_dir) {
    dirs.push_back(env_runtime_dir);
  }

  std::string exec_path = get_executable_path();
  if (!exec_path.empty()) {
    std::filesystem::path exec_dir = std::filesystem::path(exec_path).parent_path();
    dirs.push_back(exec_dir.string());
    dirs.push_back(platform::join_path(exec_dir.string(), "runtime"));
    dirs.push_back(platform::join_path(exec_dir.string(), "openconnect"));
    dirs.push_back(platform::join_path(
        platform::join_path(exec_dir.string(), "runtime"), "openconnect"));
  }

  return dirs;
}

static std::string first_existing_file(const std::vector<std::string> &paths) {
  for (const auto &path : paths) {
    if (!path.empty() && file_exists(path))
      return path;
  }
  return "";
}

// ── Colored output ──────────────────────────────────────────────

void print_success(const std::string &msg) {
  std::cout << GREEN << "✅ " << msg << RESET << std::endl;
}

void print_error(const std::string &msg) {
  std::cerr << RED << "❌ " << msg << RESET << std::endl;
}

void print_info(const std::string &msg) {
  std::cout << CYAN << "ℹ️  " << msg << RESET << std::endl;
}

void print_warning(const std::string &msg) {
  std::cout << YELLOW << "⚠️  " << msg << RESET << std::endl;
}

void print_header(const std::string &msg) {
  std::cout << std::endl;
  std::cout << BOLD << MAGENTA << "╔══════════════════════════════════════════╗"
            << RESET << std::endl;
  std::cout << BOLD << MAGENTA << "║  " << msg;
  // Pad to fill the box width
  int pad = 40 - static_cast<int>(msg.size());
  for (int i = 0; i < pad; ++i)
    std::cout << ' ';
  std::cout << RESET << BOLD << MAGENTA << "║" << RESET << std::endl;
  std::cout << BOLD << MAGENTA << "╚══════════════════════════════════════════╝"
            << RESET << std::endl;
  std::cout << std::endl;
}

// ── Path utilities ──────────────────────────────────────────────

std::string get_home_for_uid(uid_t uid) {
  return platform::home_for_uid(static_cast<unsigned int>(uid));
}

std::string get_username_for_uid(uid_t uid) {
  return platform::username_for_uid(static_cast<unsigned int>(uid));
}

std::string get_effective_home() {
  if (!runtime_home_override.empty())
    return runtime_home_override;

  return platform::effective_home();
}

std::string expand_home(const std::string &path) {
  return expand_home_with_base(path, get_effective_home());
}

std::string get_redirect_path() {
  return platform::redirect_path_for_home(get_effective_home());
}

std::string get_config_dir() {
  if (!runtime_config_dir_override.empty())
    return runtime_config_dir_override;

  return get_config_dir_for_home(get_effective_home());
}

std::string get_config_dir_for_uid(uid_t uid) {
  return get_config_dir_for_home(get_home_for_uid(uid));
}

void set_runtime_path_override(const std::string &home,
                               const std::string &config_dir) {
  runtime_home_override = home;
  runtime_config_dir_override = config_dir.empty()
                                    ? get_config_dir_for_home(home)
                                    : expand_home_with_base(config_dir, home);
}

void clear_runtime_path_override() {
  runtime_home_override.clear();
  runtime_config_dir_override.clear();
}

void set_runtime_owner(uid_t uid, gid_t gid) {
  runtime_owner_uid = uid;
  runtime_owner_gid = gid;
}

void clear_runtime_owner() {
  runtime_owner_uid = static_cast<uid_t>(-1);
  runtime_owner_gid = static_cast<gid_t>(-1);
}

bool has_runtime_owner() {
  return runtime_owner_uid != static_cast<uid_t>(-1) &&
         runtime_owner_gid != static_cast<gid_t>(-1);
}

uid_t get_runtime_owner_uid() { return runtime_owner_uid; }

gid_t get_runtime_owner_gid() { return runtime_owner_gid; }

bool sync_owner(const std::string &path) {
  if (!has_runtime_owner())
    return true;
  if (!file_exists(path))
    return false;
  return platform::sync_owner(path, static_cast<unsigned int>(runtime_owner_uid),
                              static_cast<unsigned int>(runtime_owner_gid));
}

bool set_config_dir(const std::string &dir) {
  std::string expanded = expand_home(dir);
  if (!ensure_dir(expanded))
    return false;
  // Write redirect file
  std::ofstream wf(get_redirect_path());
  if (!wf.is_open())
    return false;
  wf << dir; // store as-is (may contain ~)
  return wf.good() && sync_owner(get_redirect_path());
}

std::string get_config_path() {
  return platform::config_path(get_config_dir());
}

std::string get_pid_path() {
  return platform::pid_path(get_config_dir());
}

std::string get_log_path() {
  return platform::log_path(get_config_dir());
}

std::string get_tunnel_path() {
  return platform::tunnel_path(get_config_dir());
}

std::string get_supervisor_pid_path() {
  return platform::supervisor_pid_path(get_config_dir());
}

std::string get_route_ready_path() {
  return platform::route_ready_path(get_config_dir());
}

// ── File utilities ──────────────────────────────────────────────

bool file_exists(const std::string &path) {
#ifndef _WIN32
  struct stat st;
  return stat(path.c_str(), &st) == 0;
#else
  struct _stat st;
  return _stat(path.c_str(), &st) == 0;
#endif
}

bool fix_config_dir_ownership() {
  return platform::fix_config_dir_ownership(get_config_dir(),
                                            get_effective_home());
}

bool ensure_dir(const std::string &path) {
#ifndef _WIN32
  struct stat st;
  if (stat(path.c_str(), &st) == 0) {
    return S_ISDIR(st.st_mode) && sync_owner(path);
  }
  return mkdir(path.c_str(), 0755) == 0 && sync_owner(path);
#else
  struct _stat st;
  if (_stat(path.c_str(), &st) == 0) {
    return (st.st_mode & _S_IFDIR) != 0 && sync_owner(path);
  }
  return _mkdir(path.c_str()) == 0 && sync_owner(path);
#endif
}

std::string read_file(const std::string &path) {
  std::ifstream ifs(path);
  if (!ifs.is_open())
    return "";
  std::ostringstream ss;
  ss << ifs.rdbuf();
  return ss.str();
}

bool write_file(const std::string &path, const std::string &content) {
  std::ofstream ofs(path);
  if (!ofs.is_open())
    return false;
  ofs << content;
  return ofs.good() && sync_owner(path);
}

// ── System checks ───────────────────────────────────────────────

std::string get_bundled_runtime_dir() {
  std::vector<std::string> dirs = candidate_runtime_dirs();
  for (const auto &dir : dirs) {
    if (!dir.empty() && file_exists(dir))
      return dir;
  }
  return "";
}

std::string get_bundled_openconnect_path() {
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (dir.empty())
      continue;
#ifdef _WIN32
  candidates.push_back(platform::join_path(dir, "openconnect.exe"));
#else
  candidates.push_back(platform::join_path(dir, "openconnect"));
#endif
  }
  return first_existing_file(candidates);
}

std::string get_bundled_wintun_path() {
#ifdef _WIN32
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (dir.empty())
      continue;
    candidates.push_back(platform::join_path(dir, "wintun.dll"));
  }
  return first_existing_file(candidates);
#else
  return "";
#endif
}

std::string get_bundled_tap_installer_path() {
#ifdef _WIN32
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (dir.empty())
      continue;
    candidates.push_back(platform::join_path(dir, "tap-windows-installer.exe"));
    candidates.push_back(platform::join_path(dir, "tap-windows-amd64.exe"));
    candidates.push_back(platform::join_path(dir, "tap-windows-x86.exe"));
    candidates.push_back(platform::join_path(dir, "tap-windows/OemVista.inf"));
    candidates.push_back(platform::join_path(dir, "tap/OemVista.inf"));
  }
  return first_existing_file(candidates);
#else
  return "";
#endif
}

std::string get_openconnect_path(const std::string &runtime_mode) {
  const char *env_openconnect = std::getenv("ECNUVPN_OPENCONNECT");
  if (env_openconnect && *env_openconnect && file_exists(env_openconnect))
    return env_openconnect;

  if (runtime_mode != "system") {
    std::string bundled = get_bundled_openconnect_path();
    if (!bundled.empty()) {
#ifdef __APPLE__
      std::string verify_cmd = "codesign --verify --strict " +
                               shell_quote(bundled) + " >/dev/null 2>&1";
      if (std::system(verify_cmd.c_str()) != 0)
        bundled.clear();
#endif
    }
    if (!bundled.empty())
      return bundled;
  }

#ifdef __APPLE__
  const char *candidates[] = {"/opt/homebrew/bin/openconnect",
                              "/usr/local/bin/openconnect",
                              "/usr/bin/openconnect",
                              "/bin/openconnect"};
#elif defined(_WIN32)
  const char *candidates[] = {
      "C:\\Program Files\\OpenConnect\\openconnect.exe",
      "C:\\Program Files (x86)\\OpenConnect\\openconnect.exe",
      "openconnect.exe"};
#else
  const char *candidates[] = {"/usr/sbin/openconnect",
                              "/usr/bin/openconnect",
                              "/sbin/openconnect",
                              "/usr/local/bin/openconnect"};
#endif
  for (const char *candidate : candidates) {
#ifdef _WIN32
    if (candidate && _access(candidate, 0) == 0)
#else
    if (candidate && access(candidate, X_OK) == 0)
#endif
      return candidate;
  }

#ifdef _WIN32
  std::string resolved = trim(run_command_output("where openconnect.exe 2>nul"));
  std::string::size_type newline = resolved.find_first_of("\r\n");
  if (newline != std::string::npos)
    resolved.resize(newline);
  if (!resolved.empty() && _access(resolved.c_str(), 0) == 0)
    return resolved;
#else
  std::string resolved =
      trim(run_command_output("command -v openconnect 2>/dev/null"));
  if (!resolved.empty() && access(resolved.c_str(), X_OK) == 0)
    return resolved;
#endif
  return "";
}

bool check_openconnect(const std::string &runtime_mode) {
  return !get_openconnect_path(runtime_mode).empty();
}

bool check_root() {
#ifndef _WIN32
  return geteuid() == 0;
#else
  // On Windows, check if the process is running as Administrator
  SID_IDENTIFIER_AUTHORITY nt_auth = SECURITY_NT_AUTHORITY;
  PSID admin_group = nullptr;
  BOOL is_admin = FALSE;
  if (AllocateAndInitializeSid(&nt_auth, 2,
        SECURITY_BUILTIN_DOMAIN_RID, DOMAIN_ALIAS_RID_ADMINS,
        0, 0, 0, 0, 0, 0, &admin_group)) {
    CheckTokenMembership(nullptr, admin_group, &is_admin);
    FreeSid(admin_group);
  }
  return is_admin ? true : false;
#endif
}

bool get_interface_traffic(const std::string &iface,
                            uint64_t *rx_bytes, uint64_t *tx_bytes) {
  if (iface.empty() || !rx_bytes || !tx_bytes)
    return false;
  *rx_bytes = 0;
  *tx_bytes = 0;

#ifdef __APPLE__
  std::string output = run_command_output(
      "netstat -b -I " + shell_quote(iface) + " 2>/dev/null");
  std::istringstream stream(output);
  std::string header, data;
  if (!std::getline(stream, header) || !std::getline(stream, data))
    return false;

  // Find Ibytes and Obytes column positions from the header line
  auto col_pos = [](const std::string &h, const std::string &col) -> size_t {
    size_t pos = h.find(col);
    return (pos == std::string::npos) ? 0 : pos;
  };

  size_t ibytes_pos = col_pos(header, "Ibytes");
  size_t obytes_pos = col_pos(header, "Obytes");
  if (ibytes_pos == 0 || obytes_pos == 0)
    return false;

  // Extract the field at a given column position
  auto field_at = [](const std::string &line, size_t pos) -> std::string {
    size_t start = pos;
    while (start > 0 && line[start - 1] != ' ')
      start--;
    size_t end = pos;
    while (end < line.size() && line[end] != ' ')
      end++;
    return line.substr(start, end - start);
  };

  try {
    std::string rx = field_at(data, ibytes_pos);
    std::string tx = field_at(data, obytes_pos);
    if (!rx.empty())
      *rx_bytes = std::stoull(rx);
    if (!tx.empty())
      *tx_bytes = std::stoull(tx);
    return true;
  } catch (...) {
    return false;
  }
#elif defined(_WIN32)
  // Windows traffic counters are not required for connection control.
  // Keep status functional and report zero counters until a stable adapter
  // lookup implementation is added for both MSVC and MinGW.
  (void)iface;
  *rx_bytes = 0;
  *tx_bytes = 0;
  return false;
#else
  // Linux: read from sysfs
  auto read_sysfs_counter = [](const std::string &path) -> uint64_t {
    std::ifstream ifs(path);
    if (!ifs.is_open()) return 0;
    std::string val;
    std::getline(ifs, val);
    try { return std::stoull(val); } catch (...) { return 0; }
  };

  std::string base = "/sys/class/net/" + iface + "/statistics/";
  *rx_bytes = read_sysfs_counter(base + "rx_bytes");
  *tx_bytes = read_sysfs_counter(base + "tx_bytes");
  return (*rx_bytes > 0 || *tx_bytes > 0);
#endif
}

int run_command(const std::string &cmd) { return system(cmd.c_str()); }

std::string get_executable_path() {
#ifdef __APPLE__
  uint32_t size = 0;
  _NSGetExecutablePath(nullptr, &size);
  if (size == 0)
    return "";

  std::vector<char> buffer(size);
  if (_NSGetExecutablePath(buffer.data(), &size) != 0)
    return "";

  std::vector<char> resolved(size + 1, '\0');
  if (realpath(buffer.data(), resolved.data()))
    return resolved.data();

  return buffer.data();
#elif defined(_WIN32)
    char buf[MAX_PATH];
    DWORD len = GetModuleFileNameA(NULL, buf, MAX_PATH);
    if (len == 0 || len == MAX_PATH) return "";
    return std::string(buf);
#else
  char buf[4096];
  ssize_t len = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
  if (len <= 0) return "";
  buf[len] = '\0';
  return std::string(buf);
#endif
}

std::string run_command_output(const std::string &cmd) {
  std::array<char, 256> buffer;
  std::string result;
#ifndef _WIN32
  std::unique_ptr<FILE, decltype(&pclose)> pipe(popen(cmd.c_str(), "r"),
                                                pclose);
#else
  std::unique_ptr<FILE, decltype(&_pclose)> pipe(_popen(cmd.c_str(), "r"),
                                                 _pclose);
#endif
  if (!pipe)
    return "";
  while (fgets(buffer.data(), static_cast<int>(buffer.size()), pipe.get()) !=
         nullptr) {
    result += buffer.data();
  }
  return result;
}

std::string shell_quote(const std::string &value) {
#ifndef _WIN32
  std::string quoted = "'";
  for (char c : value) {
    if (c == '\'')
      quoted += "'\\''";
    else
      quoted += c;
  }
  quoted += "'";
  return quoted;
#else
  // Windows cmd.exe: double-quote with internal escaping
  std::string quoted = "\"";
  for (char c : value) {
    if (c == '\"')
      quoted += "\\\"";
    else if (c == '%')
      quoted += "%%";
    else
      quoted += c;
  }
  quoted += "\"";
  return quoted;
#endif
}

std::vector<std::string> split_lines(const std::string &text) {
  std::vector<std::string> lines;
  std::istringstream stream(text);
  std::string line;
  while (std::getline(stream, line)) {
    line = trim(line);
    if (!line.empty())
      lines.push_back(line);
  }
  return lines;
}

// ── String utilities ────────────────────────────────────────────

std::string trim(const std::string &s) {
  auto start = s.find_first_not_of(" \t\n\r");
  if (start == std::string::npos)
    return "";
  auto end = s.find_last_not_of(" \t\n\r");
  return s.substr(start, end - start + 1);
}

} // namespace utils
} // namespace ecnuvpn
