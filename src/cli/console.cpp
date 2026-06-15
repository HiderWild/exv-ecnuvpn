#include "cli/console.hpp"

#include <iostream>

#ifdef _WIN32
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0600
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#endif

namespace ecnuvpn::cli {

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
  int pad = 40 - static_cast<int>(msg.size());
  for (int i = 0; i < pad; ++i) {
    std::cout << ' ';
  }
  std::cout << RESET << BOLD << MAGENTA << "║" << RESET << std::endl;
  std::cout << BOLD << MAGENTA << "╚══════════════════════════════════════════╝"
            << RESET << std::endl;
  std::cout << std::endl;
}

void enable_windows_ansi() {
#ifdef _WIN32
  SetConsoleOutputCP(CP_UTF8);
  SetConsoleCP(CP_UTF8);
  HANDLE hOut = GetStdHandle(STD_OUTPUT_HANDLE);
  if (hOut != INVALID_HANDLE_VALUE) {
    DWORD mode = 0;
    if (GetConsoleMode(hOut, &mode)) {
      SetConsoleMode(hOut, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
    }
  }
#endif
}

} // namespace ecnuvpn::cli

