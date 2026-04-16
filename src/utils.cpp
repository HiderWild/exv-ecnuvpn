#include "utils.hpp"

#include <array>
#include <cstdlib>
#include <fstream>
#include <iostream>
#include <memory>
#include <pwd.h>
#include <sstream>
#include <sys/stat.h>
#include <unistd.h>

namespace ecnuvpn {
namespace utils {

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

std::string get_effective_home() {
  const char *sudo_user = getenv("SUDO_USER");
  if (sudo_user && *sudo_user) {
    struct passwd *pw = getpwnam(sudo_user);
    if (pw && pw->pw_dir)
      return pw->pw_dir;
  }

  const char *home = getenv("HOME");
  if (home && *home)
    return home;

  struct passwd *pw = getpwuid(getuid());
  if (pw && pw->pw_dir)
    return pw->pw_dir;

  return "";
}

std::string expand_home(const std::string &path) {
  if (!path.empty() && path[0] == '~') {
    std::string home = get_effective_home();
    if (!home.empty())
      return home + path.substr(1);
  }
  return path;
}

std::string get_redirect_path() { return expand_home("~/.ecnuvpn_home"); }

std::string get_config_dir() {
  // Check for redirect file written during advanced setup
  std::string redirect = get_redirect_path();
  std::ifstream rf(redirect);
  if (rf.is_open()) {
    std::string dir;
    std::getline(rf, dir);
    dir = trim(dir);
    if (!dir.empty())
      return expand_home(dir);
  }
  return expand_home("~/.ecnuvpn");
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
  return true;
}

std::string get_config_path() { return get_config_dir() + "/config.json"; }

std::string get_pid_path() { return get_config_dir() + "/ecnuvpn.pid"; }

std::string get_log_path() { return get_config_dir() + "/ecnuvpn.log"; }

std::string get_tunnel_path() { return get_config_dir() + "/tunnel.sh"; }

std::string get_supervisor_pid_path() {
  return get_config_dir() + "/ecnuvpn-supervisor.pid";
}

std::string get_route_ready_path() {
  return get_config_dir() + "/route-ready";
}

// ── File utilities ──────────────────────────────────────────────

bool file_exists(const std::string &path) {
  struct stat st;
  return stat(path.c_str(), &st) == 0;
}

bool ensure_dir(const std::string &path) {
  struct stat st;
  if (stat(path.c_str(), &st) == 0) {
    return S_ISDIR(st.st_mode);
  }
  return mkdir(path.c_str(), 0755) == 0;
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
  return ofs.good();
}

// ── System checks ───────────────────────────────────────────────

bool check_openconnect() {
  return system("which openconnect > /dev/null 2>&1") == 0;
}

bool check_root() { return geteuid() == 0; }

int run_command(const std::string &cmd) { return system(cmd.c_str()); }

std::string run_command_output(const std::string &cmd) {
  std::array<char, 256> buffer;
  std::string result;
  std::unique_ptr<FILE, decltype(&pclose)> pipe(popen(cmd.c_str(), "r"),
                                                pclose);
  if (!pipe)
    return "";
  while (fgets(buffer.data(), static_cast<int>(buffer.size()), pipe.get()) !=
         nullptr) {
    result += buffer.data();
  }
  return result;
}

std::string shell_quote(const std::string &value) {
  std::string quoted = "'";
  for (char c : value) {
    if (c == '\'')
      quoted += "'\\''";
    else
      quoted += c;
  }
  quoted += "'";
  return quoted;
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
