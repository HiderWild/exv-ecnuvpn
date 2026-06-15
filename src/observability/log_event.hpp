#pragma once

#include "observability/log_level.hpp"

#include <chrono>
#include <cstdint>
#include <string>
#include <utility>
#include <vector>

namespace exv::observability {

struct LogEvent {
  std::chrono::system_clock::time_point timestamp{};
  LogLevel level = LogLevel::Info;
  std::string component;
  std::string code;
  std::string message;
  std::vector<std::pair<std::string, std::string>> fields;
  std::string process_role;
  std::uint64_t thread_id_hash = 0;
};

} // namespace exv::observability
