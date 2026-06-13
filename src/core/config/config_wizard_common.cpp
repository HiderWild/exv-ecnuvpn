#include "core/config/config.hpp"
#include "utils.hpp"

#include <algorithm>
#include <cctype>
#include <iostream>
#include <sstream>
#include <vector>
#ifndef _WIN32
#include <termios.h>
#include <unistd.h>
#else
#include <windows.h>
#include <conio.h>
#endif

namespace ecnuvpn {
namespace config {
namespace wizard {

// Repeat a multi-byte string N times
std::string repeat_str(const std::string &s, int n) {
  std::string r;
  r.reserve(s.size() * n);
  for (int i = 0; i < n; ++i) r += s;
  return r;
}

void wiz_banner() {
  std::cout << std::endl;
  std::cout << utils::BOLD << utils::CYAN
            << "  +------------------------------------------+" << std::endl
            << "  |          EXV First-Run Setup             |" << std::endl
            << "  +------------------------------------------+" << utils::RESET
            << std::endl << std::endl;
}

void wiz_progress(int step, int total) {
  constexpr int BAR = 24;
  int filled = (step * BAR) / total;
  std::cout << utils::DIM << "  Progress: [" << utils::RESET << utils::CYAN;
  for (int i = 0; i < BAR; ++i)
    std::cout << (i < filled ? "#" : "-");
  std::cout << utils::RESET << utils::DIM << "]  " << step << "/" << total
            << utils::RESET << std::endl << std::endl;
}

void wiz_step(int step, int total, const std::string &title) {
  std::cout << std::endl;
  std::cout << utils::BOLD << utils::YELLOW << "  +- Step " << step << " / "
            << total << " -- " << title << utils::RESET << std::endl;
  wiz_progress(step, total);
}

std::string wiz_prompt(const std::string &label, const std::string &default_val) {
  std::cout << "    " << label;
  if (!default_val.empty())
    std::cout << utils::DIM << " [" << default_val << "]" << utils::RESET;
  std::cout << ": ";
  std::string input;
  std::getline(std::cin, input);
  input = utils::trim(input);
  return input.empty() ? default_val : input;
}

bool wiz_confirm(const std::string &question, bool default_yes) {
  std::cout << "    " << question << (default_yes ? " [Y/n]: " : " [y/N]: ");
  std::string input;
  std::getline(std::cin, input);
  input = utils::trim(input);
  if (input.empty()) return default_yes;
  return (input[0] == 'y' || input[0] == 'Y');
}

bool is_valid_cidr(const std::string &s) {
  if (s.empty()) return false;
  if (s.back() == '.' || s.back() == '/') return false;
  std::string ip_part = s;
  auto slash = s.find('/');
  if (slash != std::string::npos) {
    std::string pstr = s.substr(slash + 1);
    if (pstr.empty()) return false;
    int prefix;
    try { prefix = std::stoi(pstr); } catch (...) { return false; }
    if (prefix < 0 || prefix > 32) return false;
    if (pstr.size() > 1 && pstr[0] == '0') return false;
    ip_part = s.substr(0, slash);
  }
  int octets = 0;
  std::istringstream iss(ip_part);
  std::string octet;
  while (std::getline(iss, octet, '.')) {
    if (octet.empty()) return false;
    for (char c : octet)
      if (!std::isdigit(static_cast<unsigned char>(c))) return false;
    int v;
    try { v = std::stoi(octet); } catch (...) { return false; }
    if (v < 0 || v > 255) return false;
    ++octets;
  }
  return octets == 4;
}

} // namespace wizard
} // namespace config
} // namespace ecnuvpn
