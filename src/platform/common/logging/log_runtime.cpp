#include "platform/common/logging/log_runtime.hpp"

#include "observability/log_facade.hpp"
#include "platform/common/logging/file_log_sink.hpp"
#include "platform/common/logging/stdout_log_sink.hpp"
#include "platform/common/runtime_paths.hpp"

#include <iostream>
#include <mutex>
#include <string>

namespace exv::platform::logging {

namespace {

std::mutex &default_logging_mutex() {
  static std::mutex mutex;
  return mutex;
}

bool &default_logging_configured() {
  static bool configured = false;
  return configured;
}

bool &default_logging_stdout_enabled() {
  static bool stdout_enabled = false;
  return stdout_enabled;
}

} // namespace

std::shared_ptr<exv::observability::LogService>
create_default_log_service(bool emit_stdout_events) {
  auto service = std::make_shared<exv::observability::LogService>();
  service->add_sink(std::make_shared<FileLogSink>(
      exv::platform::get_log_path(), [](const std::string &path) {
        return exv::platform::sync_owner(path);
      }));
  if (emit_stdout_events) {
    service->add_sink(std::make_shared<StdoutLogSink>(std::cout));
  }
  return service;
}

void configure_default_logging(bool emit_stdout_events) {
  bool should_configure = false;
  bool stdout_enabled = false;
  {
    std::lock_guard<std::mutex> lock(default_logging_mutex());
    auto &configured = default_logging_configured();
    auto &current_stdout_enabled = default_logging_stdout_enabled();
    if (!configured || (emit_stdout_events && !current_stdout_enabled)) {
      configured = true;
      current_stdout_enabled = current_stdout_enabled || emit_stdout_events;
      should_configure = true;
    }
    stdout_enabled = current_stdout_enabled;
  }

  if (!should_configure) {
    return;
  }

  exv::observability::LogFacade::configure(
      create_default_log_service(stdout_enabled));
}

void shutdown_default_logging() {
  {
    std::lock_guard<std::mutex> lock(default_logging_mutex());
    default_logging_configured() = false;
    default_logging_stdout_enabled() = false;
  }
  exv::observability::LogFacade::shutdown();
}

} // namespace exv::platform::logging
