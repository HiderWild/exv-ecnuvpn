module;

#include "observability/log_event.hpp"
#include "observability/log_level.hpp"
#include "observability/log_sink.hpp"

export module exv.observability.log;

export namespace exv::observability {

using ::exv::observability::LogEvent;
using ::exv::observability::LogLevel;
using ::exv::observability::LogSink;
using ::exv::observability::is_high_severity;
using ::exv::observability::log_level_from_string;
using ::exv::observability::to_string;

} // namespace exv::observability
