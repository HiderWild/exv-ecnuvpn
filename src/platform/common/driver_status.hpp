#pragma once

#include "config.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace platform {

nlohmann::json driver_status_json(const Config &cfg);
nlohmann::json install_driver(const Config &cfg,
                              const nlohmann::json &payload);

} // namespace platform
} // namespace ecnuvpn