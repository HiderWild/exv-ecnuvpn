#include "platform/common/logging/file_log_sink.hpp"

#include "observability/log_level.hpp"

#include <chrono>
#include <ctime>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <sstream>
#include <utility>

namespace exv::platform::logging {

namespace {

std::string render_line(const exv::observability::LogEvent &event) {
  auto timestamp = event.timestamp;
  if (timestamp == std::chrono::system_clock::time_point{}) {
    timestamp = std::chrono::system_clock::now();
  }

  const auto time = std::chrono::system_clock::to_time_t(timestamp);
  auto local_time = *std::localtime(&time);

  std::ostringstream output;
  output << '[' << std::put_time(&local_time, "%Y-%m-%d %H:%M:%S") << "] "
         << '[' << exv::observability::to_string(event.level) << "] ";
  if (!event.component.empty()) {
    output << '[' << event.component << "] ";
  }
  if (!event.code.empty()) {
    output << "code=" << event.code << ' ';
  }
  output << event.message;
  for (const auto &field : event.fields) {
    output << ' ' << field.first << '=' << field.second;
  }
  return output.str();
}

} // namespace

FileLogSink::FileLogSink(
    std::string log_path,
    std::function<bool(const std::string &)> sync_owner)
    : log_path_(std::move(log_path)), sync_owner_(std::move(sync_owner)) {}

void FileLogSink::write(const exv::observability::LogEvent &event) {
  try {
    std::lock_guard<std::mutex> lock(mutex_);
    const auto path = std::filesystem::path(log_path_);
    const auto parent = path.parent_path();
    if (!parent.empty()) {
      std::filesystem::create_directories(parent);
    }

    std::ofstream output(log_path_, std::ios::app);
    if (!output.is_open()) {
      return;
    }

    output << render_line(event) << '\n';
    output.flush();
    if (sync_owner_) {
      (void)sync_owner_(log_path_);
    }
  } catch (...) {
  }
}

void FileLogSink::flush() {}

} // namespace exv::platform::logging
