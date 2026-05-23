#pragma once

#include "platform/common/helper_client.hpp"

#include <nlohmann/json.hpp>

#include <string>

namespace ecnuvpn {
namespace platform {

struct OneshotBootstrapRequest {
  std::string helper_path;
};

struct OneshotBackend {
  bool ok = false;
  std::string code;
  std::string message;
  std::string mode = "oneshot";
  std::string transport;
  std::string endpoint;
  std::string auth_token;
  int pid = -1;
};

OneshotBackend start_oneshot_helper(const OneshotBootstrapRequest &request);
nlohmann::json oneshot_backend_to_json(const OneshotBackend &backend);

} // namespace platform
} // namespace ecnuvpn
