#pragma once

#include <sys/types.h>
#include <cstdint>

#ifdef _WIN32
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0600
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>
#include <windows.h>
using uid_t = unsigned int;
using gid_t = unsigned int;
#endif

#include <string>
#include <vector>

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
std::string get_redirect_path();
std::string get_config_dir();
bool set_config_dir(const std::string &dir);
std::string get_config_path();
std::string get_log_path();
std::string get_tunnel_path();
std::string get_effective_home();
std::string get_home_for_uid(uid_t uid);
std::string get_username_for_uid(uid_t uid);
std::string get_config_dir_for_uid(uid_t uid);
void set_runtime_path_override(const std::string &home,
                                const std::string &config_dir);
void clear_runtime_path_override();
void set_runtime_owner(uid_t uid, gid_t gid);
void clear_runtime_owner();
bool has_runtime_owner();
uid_t get_runtime_owner_uid();
gid_t get_runtime_owner_gid();
bool sync_owner(const std::string &path);
std::string get_executable_path();

bool fix_config_dir_ownership();

// File utilities
bool file_exists(const std::string &path);
bool ensure_dir(const std::string &path);
std::string read_file(const std::string &path);
bool write_file(const std::string &path, const std::string &content);

// System checks
std::string get_bundled_runtime_dir();
std::string get_bundled_openconnect_path();
std::string get_bundled_wintun_path();
std::string get_bundled_tap_installer_path();
std::string get_openconnect_path(const std::string &runtime_mode = "auto");
bool check_openconnect(const std::string &runtime_mode = "auto");
bool check_root();
bool get_interface_traffic(const std::string &iface,
                            uint64_t *rx_bytes, uint64_t *tx_bytes);
int run_command(const std::string &cmd);
std::string run_command_output(const std::string &cmd);
std::string shell_quote(const std::string &value);
std::vector<std::string> split_lines(const std::string &text);

// String utilities
std::string trim(const std::string &s);

#ifdef _WIN32
std::wstring wide_from_utf8(const std::string &value);
std::string utf8_from_wide(const std::wstring &value);
std::string windows_error_message(unsigned long error_code);

inline void enable_windows_ansi() {
  SetConsoleOutputCP(CP_UTF8);
  SetConsoleCP(CP_UTF8);
  HANDLE hOut = GetStdHandle(STD_OUTPUT_HANDLE);
  if (hOut != INVALID_HANDLE_VALUE) {
    DWORD mode = 0;
    if (GetConsoleMode(hOut, &mode)) {
      SetConsoleMode(hOut, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
    }
  }
}
#else
inline void enable_windows_ansi() {}
#endif

} // namespace utils
} // namespace ecnuvpn
