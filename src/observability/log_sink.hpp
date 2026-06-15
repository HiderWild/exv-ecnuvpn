#pragma once

#include "observability/log_event.hpp"

namespace exv::observability {

class LogSink {
public:
  virtual ~LogSink() = default;

  virtual void write(const LogEvent &event) = 0;
  virtual void flush() {}
  virtual void shutdown() {}
};

} // namespace exv::observability
