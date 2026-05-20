#pragma once

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace platform {

inline constexpr const char *kHelperUnavailableCode = "helper_unavailable";

nlohmann::json send_helper_request(const nlohmann::json &request);

} // namespace platform
} // namespace ecnuvpn