#include "cli/core_request_formatter.hpp"

namespace exv::cli {

std::string format_core_request(const std::string &action,
                                const nlohmann::json &payload) {
  static int next_request_id = 1;

  nlohmann::json request;
  request["action"] = action;
  request["payload_json"] = payload.dump();
  request["request_id"] = "cli-" + std::to_string(next_request_id++);
  return request.dump();
}

} // namespace exv::cli
