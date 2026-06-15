#pragma once

#include <string_view>

namespace exv::observability {

enum class LogLevel {
  Trace,
  Debug,
  Info,
  Warn,
  Error,
  Fatal,
};

constexpr bool is_high_severity(LogLevel level) noexcept {
  return level == LogLevel::Warn || level == LogLevel::Error ||
         level == LogLevel::Fatal;
}

constexpr const char *to_string(LogLevel level) noexcept {
  switch (level) {
  case LogLevel::Trace:
    return "TRACE";
  case LogLevel::Debug:
    return "DEBUG";
  case LogLevel::Info:
    return "INFO";
  case LogLevel::Warn:
    return "WARN";
  case LogLevel::Error:
    return "ERROR";
  case LogLevel::Fatal:
    return "FATAL";
  }
  return "INFO";
}

constexpr LogLevel log_level_from_string(std::string_view level) noexcept {
  if (level == "TRACE" || level == "trace") {
    return LogLevel::Trace;
  }
  if (level == "DEBUG" || level == "debug") {
    return LogLevel::Debug;
  }
  if (level == "WARN" || level == "warn" || level == "WARNING" ||
      level == "warning") {
    return LogLevel::Warn;
  }
  if (level == "ERROR" || level == "error") {
    return LogLevel::Error;
  }
  if (level == "FATAL" || level == "fatal") {
    return LogLevel::Fatal;
  }
  return LogLevel::Info;
}

} // namespace exv::observability
