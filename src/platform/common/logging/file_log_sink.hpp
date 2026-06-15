#pragma once

#include "observability/log_sink.hpp"

#include <functional>
#include <mutex>
#include <string>

namespace ecnuvpn::platform::logging {

class FileLogSink final : public exv::observability::LogSink {
public:
  explicit FileLogSink(
      std::string log_path,
      std::function<bool(const std::string &)> sync_owner = {});

  void write(const exv::observability::LogEvent &event) override;
  void flush() override;

private:
  std::string log_path_;
  std::function<bool(const std::string &)> sync_owner_;
  std::mutex mutex_;
};

} // namespace ecnuvpn::platform::logging
