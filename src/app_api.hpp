#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace ecnuvpn {
namespace app_api {

// Machine-readable API used by Electron and by future non-HTTP integrations.
// Returns a JSON object/array compatible with the existing WebUI contracts.
nlohmann::json handle_action(const std::string &action,
                             const nlohmann::json &payload = nlohmann::json::object());

} // namespace app_api
} // namespace ecnuvpn
