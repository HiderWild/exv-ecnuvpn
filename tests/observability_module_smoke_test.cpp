#include <iostream>
#include <string>

import exv.observability.log;

int main() {
  exv::observability::LogEvent event;
  event.level = exv::observability::LogLevel::Info;
  event.message = "module smoke";

  if (exv::observability::to_string(event.level) != std::string("INFO")) {
    std::cerr << "unexpected log level string\n";
    return 1;
  }

  if (exv::observability::log_level_from_string("error") !=
      exv::observability::LogLevel::Error) {
    std::cerr << "unexpected parsed log level\n";
    return 1;
  }

  std::cout << "observability_module_smoke_test passed\n";
  return 0;
}
