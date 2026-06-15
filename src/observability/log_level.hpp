#pragma once

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

} // namespace exv::observability
