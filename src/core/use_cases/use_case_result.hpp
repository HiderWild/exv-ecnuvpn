#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace exv::core {

struct UseCaseResult {
  bool success = false;
  nlohmann::json payload = nlohmann::json::object();
  std::string error_code;
  std::string error_message;

  static UseCaseResult ok(nlohmann::json data = nlohmann::json::object()) {
    UseCaseResult result;
    result.success = true;
    result.payload = std::move(data);
    return result;
  }

  static UseCaseResult fail(std::string code, std::string message) {
    UseCaseResult result;
    result.success = false;
    result.error_code = std::move(code);
    result.error_message = std::move(message);
    return result;
  }
};

} // namespace exv::core
