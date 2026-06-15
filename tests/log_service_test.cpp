#include "observability/log_facade.hpp"
#include "observability/log_service.hpp"

#include <iostream>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

class MemorySink final : public exv::observability::LogSink {
public:
  void write(const exv::observability::LogEvent &event) override {
    std::lock_guard<std::mutex> lock(mutex_);
    messages_.push_back(event.message);
  }

  void flush() override {
    std::lock_guard<std::mutex> lock(mutex_);
    flushed_ = true;
  }

  void shutdown() override {
    std::lock_guard<std::mutex> lock(mutex_);
    shutdown_ = true;
  }

  std::vector<std::string> messages() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return messages_;
  }

  bool flushed() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return flushed_;
  }

  bool shutdown_called() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return shutdown_;
  }

private:
  mutable std::mutex mutex_;
  std::vector<std::string> messages_;
  bool flushed_ = false;
  bool shutdown_ = false;
};

class ThrowingSink final : public exv::observability::LogSink {
public:
  void write(const exv::observability::LogEvent &) override {
    throw std::runtime_error("sink failure");
  }
};

exv::observability::LogEvent make_event(std::string message) {
  exv::observability::LogEvent event;
  event.level = exv::observability::LogLevel::Info;
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

} // namespace

int main() {
  bool ok = true;

  {
    auto sink = std::make_shared<MemorySink>();
    exv::observability::LogService service(8);
    service.add_sink(sink);
    service.start();
    ok &= expect(service.submit(make_event("drain-me")),
                 "submit should succeed while service is running");
    service.stop();
    ok &= expect((sink->messages() == std::vector<std::string>{"drain-me"}),
                 "stop must drain queued events");
    ok &= expect(sink->flushed(), "stop must flush sinks");
    ok &= expect(sink->shutdown_called(), "stop must shutdown sinks");
  }

  {
    auto first = std::make_shared<MemorySink>();
    auto second = std::make_shared<MemorySink>();
    exv::observability::LogService service(8);
    service.add_sink(first);
    service.add_sink(second);
    service.start();
    service.submit(make_event("fanout"));
    service.flush();
    service.stop();
    ok &= expect((first->messages() == std::vector<std::string>{"fanout"}),
                 "first sink should receive event");
    ok &= expect((second->messages() == std::vector<std::string>{"fanout"}),
                 "second sink should receive event");
  }

  {
    auto sink = std::make_shared<MemorySink>();
    exv::observability::LogService service(8);
    service.add_sink(std::make_shared<ThrowingSink>());
    service.add_sink(sink);
    service.start();
    service.submit(make_event("after-throw"));
    service.flush();
    service.stop();
    ok &= expect((sink->messages() == std::vector<std::string>{"after-throw"}),
                 "throwing sink must not block later sinks");
  }

  {
    auto sink = std::make_shared<MemorySink>();
    exv::observability::LogService service(16);
    service.add_sink(sink);
    service.start();
    for (int i = 0; i < 10; ++i) {
      service.submit(make_event("flush-" + std::to_string(i)));
    }
    service.flush();
    ok &= expect(sink->messages().size() == 10,
                 "flush must wait for queued events");
    service.stop();
  }

  {
    exv::observability::LogFacade::shutdown();
    exv::observability::LogFacade::info("facade-before-setup");
    exv::observability::LogFacade::flush();
    exv::observability::LogFacade::shutdown();
  }

  if (!ok) {
    std::cerr << "log_service_test: FAILED\n";
    return 1;
  }

  std::cout << "log_service_test: all assertions passed\n";
  return 0;
}
