#pragma once

#include <string>
#include <utility>
#include <vector>

namespace ecnuvpn {
namespace logger {

// Initialize logger (ensure log file directory exists)
void init();

// Internal: write a pre-formatted line to disk. Called only by LogRenderer.
void write(const std::string &level, const std::string &text);

// Logging functions — publish to LogEventBus (which feeds LogRenderer → disk).
void info(const std::string &msg);
void error(const std::string &msg);
void warn(const std::string &msg);

// Structured event log — publishes to LogEventBus.
void event(const std::string &level, const std::string &component,
           const std::string &code, const std::string &message,
           const std::vector<std::pair<std::string, std::string>> &fields = {});

// Display recent log lines (reads from disk — the ONLY legitimate log reader).
void show_logs(int lines = 50);

// Return the last N lines of the unified log file. Only called by logs.list RPC.
std::vector<std::string> tail(int lines = 200);

} // namespace logger
} // namespace ecnuvpn
