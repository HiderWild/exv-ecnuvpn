#include "logger.hpp"
#include "utils.hpp"

#include <ctime>
#include <deque>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <sstream>

namespace ecnuvpn {
namespace logger {

static std::string get_timestamp() {
  auto now = std::time(nullptr);
  auto *tm = std::localtime(&now);
  std::ostringstream ss;
  ss << std::put_time(tm, "%Y-%m-%d %H:%M:%S");
  return ss.str();
}

void init() { utils::ensure_dir(utils::get_config_dir()); }

static void write_log(const std::string &level, const std::string &msg) {
  std::string log_path = utils::get_log_path();
  std::ofstream ofs(log_path, std::ios::app);
  if (ofs.is_open()) {
    ofs << "[" << get_timestamp() << "] [" << level << "] " << msg << std::endl;
    ofs.flush();
    utils::sync_owner(log_path);
  }
}

void info(const std::string &msg) { write_log("INFO", msg); }

void error(const std::string &msg) { write_log("ERROR", msg); }

void warn(const std::string &msg) { write_log("WARN", msg); }

void show_logs(int lines) {
  std::string log_path = utils::get_log_path();
  if (!utils::file_exists(log_path)) {
    utils::print_info("No log file found at: " + log_path);
    return;
  }

  std::ifstream ifs(log_path);
  if (!ifs.is_open()) {
    utils::print_error("Cannot open log file: " + log_path);
    return;
  }

  // Read all lines and keep last N
  std::deque<std::string> log_lines;
  std::string line;
  while (std::getline(ifs, line)) {
    log_lines.push_back(line);
    if (static_cast<int>(log_lines.size()) > lines) {
      log_lines.pop_front();
    }
  }

  utils::print_header("EXV Logs");
  std::cout << utils::DIM << "Showing last " << log_lines.size()
            << " lines from: " << log_path << utils::RESET << std::endl;
  std::cout << std::endl;

  for (const auto &l : log_lines) {
    // Color code log lines
    if (l.find("[ERROR]") != std::string::npos) {
      std::cout << utils::RED << l << utils::RESET << std::endl;
    } else if (l.find("[WARN]") != std::string::npos) {
      std::cout << utils::YELLOW << l << utils::RESET << std::endl;
    } else {
      std::cout << l << std::endl;
    }
  }
  std::cout << std::endl;
}

} // namespace logger
} // namespace ecnuvpn
