#pragma once

#include <string>
#include <utility>
#include <vector>

namespace ecnuvpn {
namespace logger {

// Initialize logger (ensure log file directory exists)
void init();

// Logging functions
void info(const std::string &msg);
void error(const std::string &msg);
void warn(const std::string &msg);

// Structured event log. Renders a single line owned by the unified logging
// module so every component reports consistently:
//   [ts] [LEVEL] [component] code=<code> <message> key=value ...
// level is one of "INFO", "WARN", "ERROR".
void event(const std::string &level, const std::string &component,
           const std::string &code, const std::string &message,
           const std::vector<std::pair<std::string, std::string>> &fields = {});

// Display recent log lines
void show_logs(int lines = 50);

// Return the last N lines of the unified log file (newest last). Shared by
// `exv logs`, the logs.tail RPC, and the SSE log watcher so all consumers see
// identical content.
std::vector<std::string> tail(int lines = 200);

} // namespace logger
} // namespace ecnuvpn
