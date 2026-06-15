#pragma once

#include "observability/log_level.hpp"
#include "observability/log_service.hpp"

#include <memory>
#include <string>
#include <utility>
#include <vector>

namespace exv::observability {

class LogFacade {
public:
  static void configure(std::shared_ptr<LogService> service);
  static LogService &default_service();

  static void info(std::string message);
  static void warn(std::string message);
  static void error(std::string message);
  static void event(
      LogLevel level, std::string component, std::string code,
      std::string message,
      std::vector<std::pair<std::string, std::string>> fields = {});
  static void flush();
  static void shutdown();
};

} // namespace exv::observability
