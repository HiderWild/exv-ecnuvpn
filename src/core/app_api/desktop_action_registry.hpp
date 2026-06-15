#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace ecnuvpn {
namespace app_api {

nlohmann::json dispatch_desktop_action(const std::string &action,
                                       const nlohmann::json &payload);

} // namespace app_api
} // namespace ecnuvpn
