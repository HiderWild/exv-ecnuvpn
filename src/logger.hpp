#pragma once

#include <string>

namespace ecnuvpn {
namespace logger {

// Initialize logger (ensure log file directory exists)
void init();

// Logging functions
void info(const std::string &msg);
void error(const std::string &msg);
void warn(const std::string &msg);

// Display recent log lines
void show_logs(int lines = 50);

} // namespace logger
} // namespace ecnuvpn
