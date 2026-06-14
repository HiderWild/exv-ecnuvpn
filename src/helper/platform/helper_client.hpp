#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace ecnuvpn {
namespace platform {

inline constexpr const char *kHelperUnavailableCode = "helper_unavailable";

struct HelperEndpoint {
  std::string endpoint;
  std::string auth_token;
};

nlohmann::json send_helper_request(const nlohmann::json &request);
nlohmann::json send_helper_request(const HelperEndpoint &endpoint,
                                   const nlohmann::json &request);

} // namespace platform
} // namespace ecnuvpn
