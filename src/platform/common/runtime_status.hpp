#pragma once

#include "platform/common/config_view.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace platform {

nlohmann::json runtime_status_json(const ConfigView &cfg);

} // namespace platform
} // namespace ecnuvpn
