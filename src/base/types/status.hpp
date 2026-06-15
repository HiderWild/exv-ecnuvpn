#pragma once

#include <string>
#include <utility>

namespace exv::base {

struct Status {
  bool success = true;
  std::string code;
  std::string message;

  bool ok() const { return success; }

  static Status success_status() { return Status{}; }

  static Status failure(std::string code, std::string message) {
    Status status;
    status.success = false;
    status.code = std::move(code);
    status.message = std::move(message);
    return status;
  }
};

} // namespace exv::base
