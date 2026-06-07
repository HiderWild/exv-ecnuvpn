#include "log_event_bus.hpp"

namespace ecnuvpn {

LogEventBus& LogEventBus::instance() {
  static LogEventBus bus;
  return bus;
}

LogEventBus::SubscriptionId LogEventBus::subscribe(Subscriber subscriber) {
  std::lock_guard<std::mutex> lock(mutex_);
  SubscriptionId id = next_id_++;
  subscribers_[id] = std::move(subscriber);
  return id;
}

void LogEventBus::unsubscribe(SubscriptionId id) {
  std::lock_guard<std::mutex> lock(mutex_);
  subscribers_.erase(id);
}

void LogEventBus::publish(const TypedLogEvent& event) {
  std::lock_guard<std::mutex> lock(mutex_);
  for (const auto& [id, sub] : subscribers_) {
    if (sub) sub(event);
  }
}

} // namespace ecnuvpn
