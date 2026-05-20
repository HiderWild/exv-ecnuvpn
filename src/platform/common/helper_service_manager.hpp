#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace ecnuvpn {
namespace platform {

struct HelperServiceManagerContext {
  bool (*wait_until_available)(int attempts, unsigned int delay_us) = nullptr;
  bool (*send_request)(const nlohmann::json &request, nlohmann::json *response,
                       std::string *error_message,
                       int timeout_seconds) = nullptr;
  void (*clear_session_state)() = nullptr;
};

int install_helper_service(const std::string &executable_path,
                           const HelperServiceManagerContext &context);
int uninstall_helper_service(const HelperServiceManagerContext &context);
int show_helper_service_status(const HelperServiceManagerContext &context);

} // namespace platform
} // namespace ecnuvpn