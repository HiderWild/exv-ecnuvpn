#pragma once

#include "core/config/config.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace platform {

nlohmann::json runtime_status_json(const Config &cfg);

} // namespace platform
} // namespace ecnuvpn
