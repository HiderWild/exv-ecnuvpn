#include "common/diagnostics/logger.hpp"
#include "common/diagnostics/log_event_bus.hpp"
#include "observability/log_facade.hpp"
#include "cli/console.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/runtime_paths.hpp"

#include <deque>
#include <fstream>
#include <iostream>
#include <utility>
#include <vector>

namespace ecnuvpn {
namespace logger {
namespace {

exv::observability::LogLevel log_level_from_string(const std::string &level) {
  if (level == "TRACE" || level == "trace") {
    return exv::observability::LogLevel::Trace;
  }
  if (level == "DEBUG" || level == "debug") {
    return exv::observability::LogLevel::Debug;
  }
  if (level == "WARN" || level == "warn" || level == "WARNING" ||
      level == "warning") {
    return exv::observability::LogLevel::Warn;
  }
  if (level == "ERROR" || level == "error") {
    return exv::observability::LogLevel::Error;
  }
  if (level == "FATAL" || level == "fatal") {
    return exv::observability::LogLevel::Fatal;
  }
  return exv::observability::LogLevel::Info;
}

void publish_compat_event(
    const std::string &level, const std::string &component,
    const std::string &code, const std::string &message,
    const std::vector<std::pair<std::string, std::string>> &fields) {
  TypedLogEvent ev;
  ev.level = level;
  ev.component = component;
  ev.code = code;
  ev.message = message;
  ev.fields = fields;
  LogEventBus::instance().publish(ev);
}

} // namespace

void init() {
  platform::ensure_dir(platform::get_config_dir());
  platform::logging::configure_default_logging(false);
}

void write(const std::string &level, const std::string &text) {
  std::string log_path = platform::get_log_path();
  std::ofstream ofs(log_path, std::ios::app);
  if (ofs.is_open()) {
    ofs << text << std::endl;
    ofs.flush();
    platform::sync_owner(log_path);
  }
}

void info(const std::string &msg) {
  exv::observability::LogFacade::info(msg);
  publish_compat_event("INFO", "", "", msg, {});
}

void error(const std::string &msg) {
  exv::observability::LogFacade::error(msg);
  publish_compat_event("ERROR", "", "", msg, {});
}

void warn(const std::string &msg) {
  exv::observability::LogFacade::warn(msg);
  publish_compat_event("WARN", "", "", msg, {});
}

void event(const std::string &level, const std::string &component,
           const std::string &code, const std::string &message,
           const std::vector<std::pair<std::string, std::string>> &fields) {
  exv::observability::LogFacade::event(log_level_from_string(level), component,
                                       code, message, fields);
  publish_compat_event(level, component, code, message, fields);
}

std::vector<std::string> tail(int lines) {
  std::vector<std::string> result;
  std::string log_path = platform::get_log_path();
  std::ifstream ifs(log_path);
  if (!ifs.is_open()) {
    return result;
  }
  std::deque<std::string> buffer;
  std::string line;
  while (std::getline(ifs, line)) {
    buffer.push_back(line);
    if (lines > 0 && static_cast<int>(buffer.size()) > lines) {
      buffer.pop_front();
    }
  }
  result.assign(buffer.begin(), buffer.end());
  return result;
}

void show_logs(int lines) {
  std::string log_path = platform::get_log_path();
  if (!platform::file_exists(log_path)) {
    cli::print_info("No log file found at: " + log_path);
    return;
  }

  std::ifstream ifs(log_path);
  if (!ifs.is_open()) {
    cli::print_error("Cannot open log file: " + log_path);
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

  cli::print_header("EXV Logs");
  std::cout << cli::DIM << "Showing last " << log_lines.size()
            << " lines from: " << log_path << cli::RESET << std::endl;
  std::cout << std::endl;

  for (const auto &l : log_lines) {
    // Color code log lines
    if (l.find("[ERROR]") != std::string::npos) {
      std::cout << cli::RED << l << cli::RESET << std::endl;
    } else if (l.find("[WARN]") != std::string::npos) {
      std::cout << cli::YELLOW << l << cli::RESET << std::endl;
    } else {
      std::cout << l << std::endl;
    }
  }
  std::cout << std::endl;
}

} // namespace logger
} // namespace ecnuvpn
