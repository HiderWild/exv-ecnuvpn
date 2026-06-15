#include "platform/common/logging/log_runtime.hpp"

#include "observability/log_facade.hpp"
#include "platform/common/logging/file_log_sink.hpp"
#include "platform/common/logging/stdout_log_sink.hpp"
#include "platform/common/runtime_paths.hpp"

#include <iostream>
#include <string>

namespace ecnuvpn::platform::logging {

std::shared_ptr<exv::observability::LogService>
create_default_log_service(bool emit_stdout_events) {
  auto service = std::make_shared<exv::observability::LogService>();
  service->add_sink(std::make_shared<FileLogSink>(
      ecnuvpn::platform::get_log_path(), [](const std::string &path) {
        return ecnuvpn::platform::sync_owner(path);
      }));
  if (emit_stdout_events) {
    service->add_sink(std::make_shared<StdoutLogSink>(std::cout));
  }
  return service;
}

void configure_default_logging(bool emit_stdout_events) {
  exv::observability::LogFacade::configure(
      create_default_log_service(emit_stdout_events));
}

void shutdown_default_logging() { exv::observability::LogFacade::shutdown(); }

} // namespace ecnuvpn::platform::logging
