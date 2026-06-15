#pragma once

#include "core/config/config.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace app_api {

nlohmann::json frontend_status_from_helper(const nlohmann::json &helper_resp,
                                           const Config &cfg);
nlohmann::json disconnected_status(const Config &cfg);
nlohmann::json frontend_status_from_snapshot_json(const nlohmann::json &snapshot,
                                                  const Config &cfg);
nlohmann::json auth_config(const Config &cfg);
nlohmann::json settings_config(const Config &cfg);
nlohmann::json routes_json(const Config &cfg);
nlohmann::json key_status_json();
nlohmann::json service_status_json();
nlohmann::json runtime_status_json(const Config &cfg);
nlohmann::json frontend_status_from_controller_snapshot(
    const exv::core::TunnelStatusSnapshot &snap, const Config &cfg);
nlohmann::json driver_status_json(const Config &cfg);
nlohmann::json install_driver(const Config &cfg, const nlohmann::json &payload);

} // namespace app_api
} // namespace ecnuvpn
