#pragma once

#include <functional>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

namespace ecnuvpn {

struct TypedLogEvent {
  std::string level;       // "INFO", "WARN", "ERROR"
  std::string component;   // e.g. "tunnel", "engine", "helper"
  std::string code;        // machine-readable error code
  std::string message;     // human-readable message
  std::vector<std::pair<std::string, std::string>> fields; // key=value pairs
};

class LogEventBus {
public:
  using Subscriber = std::function<void(const TypedLogEvent&)>;
  using SubscriptionId = size_t;

  static LogEventBus& instance();

  SubscriptionId subscribe(Subscriber subscriber);
  void unsubscribe(SubscriptionId id);
  void publish(const TypedLogEvent& event);

private:
  LogEventBus() = default;
  std::mutex mutex_;
  std::unordered_map<SubscriptionId, Subscriber> subscribers_;
  SubscriptionId next_id_ = 1;
};

} // namespace ecnuvpn
