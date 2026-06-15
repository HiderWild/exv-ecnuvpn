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
    return "trace";
  case LogLevel::Debug:
    return "debug";
  case LogLevel::Info:
    return "info";
  case LogLevel::Warn:
    return "warn";
  case LogLevel::Error:
    return "error";
  case LogLevel::Fatal:
    return "fatal";
  }
  return "info";
}

} // namespace exv::observability
