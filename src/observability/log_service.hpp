#pragma once

#include "observability/log_queue.hpp"
#include "observability/log_sink.hpp"

#include <atomic>
#include <condition_variable>
#include <cstddef>
#include <memory>
#include <mutex>
#include <thread>
#include <vector>

namespace exv::observability {

class LogService {
public:
  explicit LogService(std::size_t queue_capacity = 4096);
  ~LogService();

  LogService(const LogService &) = delete;
  LogService &operator=(const LogService &) = delete;

  void add_sink(std::shared_ptr<LogSink> sink);
  void start();
  void stop();
  void flush();
  bool submit(LogEvent event);
  std::size_t dropped_count() const;

private:
  void worker_loop();
  void deliver(const LogEvent &event);
  void flush_sinks();
  void shutdown_sinks();
  std::vector<std::shared_ptr<LogSink>> sinks_snapshot() const;

  LogQueue queue_;
  mutable std::mutex sinks_mutex_;
  std::vector<std::shared_ptr<LogSink>> sinks_;

  mutable std::mutex lifecycle_mutex_;
  std::thread worker_;
  bool running_ = false;
  bool stopping_ = false;

  mutable std::mutex drain_mutex_;
  std::condition_variable drained_;
  std::size_t pending_events_ = 0;
  std::atomic<std::size_t> sink_failure_count_{0};
};

} // namespace exv::observability
