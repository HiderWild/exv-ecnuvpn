#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace ecnuvpn {
namespace app_api {

// Machine-readable API used by Electron and by future non-HTTP integrations.
// Returns a JSON object/array compatible with the existing WebUI contracts.
nlohmann::json handle_action(const std::string &action,
                             const nlohmann::json &payload = nlohmann::json::object());

// Returns true if a TunnelController has been initialized (i.e., the
// Core-owned mode is active). When true, vpn::start_with_password() should
// skip the native-engine supervisor path — the TunnelController manages the
// NativeVpnEngineSession lifecycle via CoreSessionRunner.
bool is_tunnel_controller_active();

} // namespace app_api
} // namespace ecnuvpn
