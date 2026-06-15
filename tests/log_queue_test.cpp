#include "observability/log_queue.hpp"

#include <atomic>
#include <chrono>
#include <iostream>
#include <string>
#include <thread>
#include <vector>

namespace {

exv::observability::LogEvent make_event(exv::observability::LogLevel level,
                                        std::string message) {
  exv::observability::LogEvent event;
  event.level = level;
  event.component = "test";
  event.message = std::move(message);
  return event;
}

bool expect(bool condition, const std::string &message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

std::vector<std::string> drain_messages(exv::observability::LogQueue &queue) {
  queue.close();
  std::vector<std::string> messages;
  exv::observability::LogEvent event;
  while (queue.pop(event)) {
    messages.push_back(event.message);
  }
  return messages;
}

} // namespace

int main() {
  using exv::observability::LogLevel;

  bool ok = true;

  {
    exv::observability::LogQueue queue(3);
    ok &= expect(queue.push(make_event(LogLevel::Info, "first")),
                 "first push should succeed");
    ok &= expect(queue.push(make_event(LogLevel::Info, "second")),
                 "second push should succeed");

    exv::observability::LogEvent event;
    ok &= expect(queue.pop(event), "first pop should succeed");
    ok &= expect(event.message == "first", "queue must preserve FIFO order");
    ok &= expect(queue.pop(event), "second pop should succeed");
    ok &= expect(event.message == "second", "second FIFO value mismatch");
  }

  {
    exv::observability::LogQueue queue(1);
    std::atomic<bool> popped{true};
    std::thread waiter([&] {
      exv::observability::LogEvent event;
      popped = queue.pop(event);
    });
    std::this_thread::sleep_for(std::chrono::milliseconds(25));
    queue.close();
    waiter.join();
    ok &= expect(!popped.load(), "close must wake waiters with empty queue");
  }

  {
    exv::observability::LogQueue queue(3);
    queue.push(make_event(LogLevel::Info, "info-a"));
    queue.push(make_event(LogLevel::Info, "info-b"));
    queue.push(make_event(LogLevel::Info, "info-c"));
    queue.push(make_event(LogLevel::Info, "info-d"));

    const auto messages = drain_messages(queue);
    ok &= expect(queue.dropped_count() == 1,
                 "overflow should count dropped low-severity events");
    ok &= expect((messages == std::vector<std::string>{"info-b", "info-c",
                                                       "info-d"}),
                 "overflow should drop oldest low-severity event");
  }

  {
    exv::observability::LogQueue queue(3);
    queue.push(make_event(LogLevel::Error, "error-a"));
    queue.push(make_event(LogLevel::Info, "info-a"));
    queue.push(make_event(LogLevel::Info, "info-b"));
    queue.push(make_event(LogLevel::Error, "error-b"));

    const auto messages = drain_messages(queue);
    ok &= expect(queue.dropped_count() == 1,
                 "high-severity overflow should drop one event");
    ok &= expect((messages == std::vector<std::string>{"error-a", "info-b",
                                                       "error-b"}),
                 "high-severity events should be retained ahead of info");
  }

  {
    exv::observability::LogQueue queue(2);
    queue.push(make_event(LogLevel::Error, "error-a"));
    queue.push(make_event(LogLevel::Error, "error-b"));
    queue.push(make_event(LogLevel::Fatal, "fatal-c"));

    const auto messages = drain_messages(queue);
    ok &= expect(queue.dropped_count() == 1,
                 "all-high overflow should still drop exactly one event");
    ok &= expect((messages == std::vector<std::string>{"error-b", "fatal-c"}),
                 "all-high overflow should drop the oldest event");
  }

  if (!ok) {
    std::cerr << "log_queue_test: FAILED\n";
    return 1;
  }

  std::cout << "log_queue_test: all assertions passed\n";
  return 0;
}
