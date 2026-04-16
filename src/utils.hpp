#pragma once

#include <string>

namespace ecnuvpn {
namespace utils {

// ANSI color codes
constexpr const char *RESET = "\033[0m";
constexpr const char *RED = "\033[31m";
constexpr const char *GREEN = "\033[32m";
constexpr const char *YELLOW = "\033[33m";
constexpr const char *BLUE = "\033[34m";
constexpr const char *MAGENTA = "\033[35m";
constexpr const char *CYAN = "\033[36m";
constexpr const char *BOLD = "\033[1m";
constexpr const char *DIM = "\033[2m";
constexpr const char *UNDERLINE = "\033[4m";
constexpr const char *REVERSE = "\033[7m";

// Colored output
void print_success(const std::string &msg);
void print_error(const std::string &msg);
void print_info(const std::string &msg);
void print_warning(const std::string &msg);
void print_header(const std::string &msg);

// Path utilities
std::string expand_home(const std::string &path);
std::string get_redirect_path(); // ~/.ecnuvpn_home (redirect file)
std::string get_config_dir();
bool set_config_dir(const std::string &dir); // write redirect + create dir
std::string get_config_path();
std::string get_pid_path();
std::string get_log_path();
std::string get_tunnel_path();
std::string get_supervisor_pid_path();
std::string get_route_ready_path();
std::string get_effective_home();

// File utilities
bool file_exists(const std::string &path);
bool ensure_dir(const std::string &path);
std::string read_file(const std::string &path);
bool write_file(const std::string &path, const std::string &content);

// System checks
bool check_openconnect();
bool check_root();
int run_command(const std::string &cmd);
std::string run_command_output(const std::string &cmd);
std::string shell_quote(const std::string &value);

// String utilities
std::string trim(const std::string &s);

} // namespace utils
} // namespace ecnuvpn
