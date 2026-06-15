#include "observability/log_service.hpp"

#include <chrono>
#include <functional>
#include <utility>

namespace exv::observability {

namespace {

void fill_runtime_fields(LogEvent &event) {
  if (event.timestamp == std::chrono::system_clock::time_point{}) {
    event.timestamp = std::chrono::system_clock::now();
  }
  if (event.thread_id_hash == 0) {
    event.thread_id_hash =
        static_cast<std::uint64_t>(std::hash<std::thread::id>{}(
            std::this_thread::get_id()));
  }
}

} // namespace

LogService::LogService(std::size_t queue_capacity) : queue_(queue_capacity) {}

LogService::~LogService() { stop(); }

void LogService::add_sink(std::shared_ptr<LogSink> sink) {
  if (!sink) {
    return;
  }
  std::lock_guard<std::mutex> lock(sinks_mutex_);
  sinks_.push_back(std::move(sink));
}

void LogService::start() {
  std::lock_guard<std::mutex> lock(lifecycle_mutex_);
  if (running_) {
    return;
  }

  stopping_ = false;
  running_ = true;
  worker_ = std::thread(&LogService::worker_loop, this);
}

void LogService::stop() {
  {
    std::lock_guard<std::mutex> lock(lifecycle_mutex_);
    if (!running_ && !worker_.joinable()) {
      return;
    }
    stopping_ = true;
    queue_.close();
  }

  if (worker_.joinable()) {
    worker_.join();
  }

  {
    std::lock_guard<std::mutex> lock(lifecycle_mutex_);
    running_ = false;
  }

  flush_sinks();
  shutdown_sinks();
}

void LogService::flush() {
  {
    std::unique_lock<std::mutex> lock(drain_mutex_);
    drained_.wait(lock, [this] { return pending_events_ == 0; });
  }
  flush_sinks();
}

bool LogService::submit(LogEvent event) {
  {
    std::lock_guard<std::mutex> lock(lifecycle_mutex_);
    if (stopping_) {
      return false;
    }
  }

  fill_runtime_fields(event);

  {
    std::lock_guard<std::mutex> lock(drain_mutex_);
    ++pending_events_;
  }

  if (queue_.push(std::move(event))) {
    return true;
  }

  {
    std::lock_guard<std::mutex> lock(drain_mutex_);
    --pending_events_;
  }
  drained_.notify_all();
  return false;
}

std::size_t LogService::dropped_count() const {
  return queue_.dropped_count();
}

void LogService::worker_loop() {
  LogEvent event;
  while (queue_.pop(event)) {
    deliver(event);
    {
      std::lock_guard<std::mutex> lock(drain_mutex_);
      if (pending_events_ > 0) {
        --pending_events_;
      }
    }
    drained_.notify_all();
  }
}

void LogService::deliver(const LogEvent &event) {
  for (const auto &sink : sinks_snapshot()) {
    try {
      sink->write(event);
    } catch (...) {
      sink_failure_count_.fetch_add(1, std::memory_order_relaxed);
    }
  }
}

void LogService::flush_sinks() {
  for (const auto &sink : sinks_snapshot()) {
    try {
      sink->flush();
    } catch (...) {
      sink_failure_count_.fetch_add(1, std::memory_order_relaxed);
    }
  }
}

void LogService::shutdown_sinks() {
  for (const auto &sink : sinks_snapshot()) {
    try {
      sink->shutdown();
    } catch (...) {
      sink_failure_count_.fetch_add(1, std::memory_order_relaxed);
    }
  }
}

std::vector<std::shared_ptr<LogSink>> LogService::sinks_snapshot() const {
  std::lock_guard<std::mutex> lock(sinks_mutex_);
  return sinks_;
}

} // namespace exv::observability
