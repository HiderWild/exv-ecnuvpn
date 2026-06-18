#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace exv::cli {

std::string format_core_request(const std::string &action,
                                const nlohmann::json &payload =
                                    nlohmann::json::object());

} // namespace exv::cli
