#pragma once

#include "observability/log_event.hpp"

#include <condition_variable>
#include <cstddef>
#include <deque>
#include <mutex>

namespace exv::observability {

class LogQueue {
public:
  explicit LogQueue(std::size_t capacity);

  bool push(LogEvent event);
  bool pop(LogEvent &out);
  void close();
  std::size_t dropped_count() const;

private:
  std::size_t capacity_;
  mutable std::mutex mutex_;
  std::condition_variable ready_;
  std::deque<LogEvent> queue_;
  std::size_t dropped_count_ = 0;
  bool closed_ = false;
};

} // namespace exv::observability
