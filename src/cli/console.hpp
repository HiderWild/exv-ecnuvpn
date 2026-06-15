#pragma once

#include <string>

namespace ecnuvpn::cli {

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

void print_success(const std::string &msg);
void print_error(const std::string &msg);
void print_info(const std::string &msg);
void print_warning(const std::string &msg);
void print_header(const std::string &msg);
void enable_windows_ansi();

} // namespace ecnuvpn::cli

