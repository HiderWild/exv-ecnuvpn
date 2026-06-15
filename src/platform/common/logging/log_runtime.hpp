#pragma once

#include "observability/log_service.hpp"

#include <memory>

namespace ecnuvpn::platform::logging {

std::shared_ptr<exv::observability::LogService>
create_default_log_service(bool emit_stdout_events);

void configure_default_logging(bool emit_stdout_events);
void shutdown_default_logging();

} // namespace ecnuvpn::platform::logging
