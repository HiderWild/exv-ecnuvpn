#include "observability/log_facade.hpp"

#include <mutex>

namespace exv::observability {

namespace {

std::mutex &service_mutex() {
  static std::mutex mutex;
  return mutex;
}

std::shared_ptr<LogService> &service_slot() {
  static std::shared_ptr<LogService> service;
  return service;
}

std::shared_ptr<LogService> current_or_create_service() {
  std::lock_guard<std::mutex> lock(service_mutex());
  auto &service = service_slot();
  if (!service) {
    service = std::make_shared<LogService>();
    service->start();
  }
  return service;
}

} // namespace

void LogFacade::configure(std::shared_ptr<LogService> service) {
  std::shared_ptr<LogService> previous;
  {
    std::lock_guard<std::mutex> lock(service_mutex());
    previous = std::move(service_slot());
    service_slot() = std::move(service);
    if (service_slot()) {
      service_slot()->start();
    }
  }

  if (previous) {
    previous->stop();
  }
}

LogService &LogFacade::default_service() {
  return *current_or_create_service();
}

void LogFacade::info(std::string message) {
  event(LogLevel::Info, "default", "", std::move(message));
}

void LogFacade::warn(std::string message) {
  event(LogLevel::Warn, "default", "", std::move(message));
}

void LogFacade::error(std::string message) {
  event(LogLevel::Error, "default", "", std::move(message));
}

void LogFacade::event(
    LogLevel level, std::string component, std::string code,
    std::string message,
    std::vector<std::pair<std::string, std::string>> fields) {
  LogEvent log_event;
  log_event.level = level;
  log_event.component = std::move(component);
  log_event.code = std::move(code);
  log_event.message = std::move(message);
  log_event.fields = std::move(fields);
  current_or_create_service()->submit(std::move(log_event));
}

void LogFacade::event(
    std::string level, std::string component, std::string code,
    std::string message,
    std::vector<std::pair<std::string, std::string>> fields) {
  event(log_level_from_string(level), std::move(component), std::move(code),
        std::move(message), std::move(fields));
}

void LogFacade::flush() {
  std::shared_ptr<LogService> service;
  {
    std::lock_guard<std::mutex> lock(service_mutex());
    service = service_slot();
  }
  if (service) {
    service->flush();
  }
}

void LogFacade::shutdown() {
  std::shared_ptr<LogService> service;
  {
    std::lock_guard<std::mutex> lock(service_mutex());
    service = std::move(service_slot());
    service_slot().reset();
  }

  if (service) {
    service->stop();
  }
}

} // namespace exv::observability
