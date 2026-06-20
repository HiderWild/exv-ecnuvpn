#pragma once

#include "observability/log_sink.hpp"

#include <iosfwd>
#include <mutex>

namespace exv::platform::logging {

class StdoutLogSink final : public exv::observability::LogSink {
public:
  explicit StdoutLogSink(std::ostream &out);

  void write(const exv::observability::LogEvent &event) override;
  void flush() override;

private:
  std::ostream &out_;
  std::mutex mutex_;
};

} // namespace exv::platform::logging
