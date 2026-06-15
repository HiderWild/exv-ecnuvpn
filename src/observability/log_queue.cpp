#include "observability/log_queue.hpp"

#include <algorithm>
#include <utility>

namespace exv::observability {

LogQueue::LogQueue(std::size_t capacity)
    : capacity_(capacity == 0 ? 1 : capacity) {}

bool LogQueue::push(LogEvent event) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (closed_) {
    return false;
  }

  if (queue_.size() >= capacity_) {
    const auto low_severity =
        std::find_if(queue_.begin(), queue_.end(), [](const LogEvent &entry) {
          return !is_high_severity(entry.level);
        });

    if (low_severity != queue_.end()) {
      queue_.erase(low_severity);
      ++dropped_count_;
    } else if (!is_high_severity(event.level)) {
      ++dropped_count_;
      return false;
    } else {
      queue_.pop_front();
      ++dropped_count_;
    }
  }

  queue_.push_back(std::move(event));
  ready_.notify_one();
  return true;
}

bool LogQueue::pop(LogEvent &out) {
  std::unique_lock<std::mutex> lock(mutex_);
  ready_.wait(lock, [this] { return closed_ || !queue_.empty(); });

  if (queue_.empty()) {
    return false;
  }

  out = std::move(queue_.front());
  queue_.pop_front();
  return true;
}

void LogQueue::close() {
  {
    std::lock_guard<std::mutex> lock(mutex_);
    closed_ = true;
  }
  ready_.notify_all();
}

std::size_t LogQueue::dropped_count() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return dropped_count_;
}

} // namespace exv::observability
