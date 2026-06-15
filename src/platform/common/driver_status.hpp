#pragma once

#include "platform/common/config_view.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace platform {

nlohmann::json driver_status_json(const ConfigView &cfg);
nlohmann::json install_driver(const ConfigView &cfg,
                              const nlohmann::json &payload);

} // namespace platform
} // namespace ecnuvpn
