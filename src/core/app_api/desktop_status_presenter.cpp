#include "core/app_api/desktop_status_presenter.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/config/config_api.hpp"
#include "core/config/config_platform_view.hpp"
#include "core/network/virtual_network_status.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"

namespace ecnuvpn {
namespace app_api {

nlohmann::json frontend_status_from_helper(const nlohmann::json &helper_resp,
                                           const Config &cfg) {
  const bool running = json_bool(helper_resp, "running", false);
  const bool network_ready = json_bool(helper_resp, "network_ready", false);
  nlohmann::json j;
  j["connected"] = running && network_ready;
  j["process_running"] = running;
  j["server"] = json_string(helper_resp, "server", cfg.server);
  j["username"] = cfg.username;
  j["pid"] = json_int(helper_resp, "pid", -1);
  j["network_ready"] = network_ready;
  j["interface"] = json_string(helper_resp, "interface");
  j["internal_ip"] = json_string(helper_resp, "internal_ip");
  j["route_count"] =
      json_int(helper_resp, "route_count", static_cast<int>(cfg.routes.size()));
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = json_u64(helper_resp, "rx_bytes", 0);
  j["tx_bytes"] = json_u64(helper_resp, "tx_bytes", 0);
  try {
    virtual_network::add_status_fields(j, json_string(j, "interface"));
  } catch (...) {
  }
  return j;
}

nlohmann::json disconnected_status(const Config &cfg) {
  nlohmann::json j{{"connected", false},
                   {"process_running", false},
                   {"server", cfg.server},
                   {"username", cfg.username},
                   {"pid", -1},
                   {"network_ready", false},
                   {"interface", ""},
                   {"internal_ip", ""},
                   {"route_count", static_cast<int>(cfg.routes.size())},
                   {"mtu", cfg.mtu},
                   {"uptime_seconds", 0},
                   {"rx_bytes", 0},
                   {"tx_bytes", 0},
                   {"upstream_virtual_detected", false},
                   {"upstream_virtual_adapters", nlohmann::json::array()},
                   {"upstream_virtual_message", ""},
                   {"route_policy", "normal"}};
  try {
    virtual_network::add_status_fields(j, "");
  } catch (...) {
  }
  return j;
}

nlohmann::json frontend_status_from_snapshot_json(
    const nlohmann::json &snapshot, const Config &cfg) {
  const std::string iface = json_string(snapshot, "interface");
  const bool running = json_bool(snapshot, "running", false);
  const bool network_ready = json_bool(snapshot, "network_ready", false);
  uint64_t rx_bytes = 0;
  uint64_t tx_bytes = 0;
  if (network_ready && !iface.empty()) {
    platform::get_interface_traffic(iface, &rx_bytes, &tx_bytes);
  }

  nlohmann::json j;
  j["connected"] = running && network_ready;
  j["process_running"] = running;
  j["server"] = cfg.server;
  j["username"] = cfg.username;
  j["pid"] = json_int(snapshot, "pid", -1);
  j["network_ready"] = network_ready;
  j["interface"] = iface;
  j["internal_ip"] = json_string(snapshot, "internal_ip");
  j["route_count"] = static_cast<int>(cfg.routes.size());
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = rx_bytes;
  j["tx_bytes"] = tx_bytes;
  try {
    virtual_network::add_status_fields(j, iface);
  } catch (...) {
  }
  return j;
}

nlohmann::json auth_config(const Config &cfg) {
  return nlohmann::json{{"server", cfg.server},
                        {"username", cfg.username},
                        {"password", ""},
                        {"password_stored", !cfg.password.empty()},
                        {"user_agent", cfg.useragent},
                        {"remember_password", cfg.remember_password}};
}

nlohmann::json settings_config(const Config &cfg) {
  std::string extra_args;
  for (size_t i = 0; i < cfg.extra_args.size(); ++i) {
    if (i > 0) {
      extra_args += " ";
    }
    extra_args += cfg.extra_args[i];
  }

  return nlohmann::json{{"mtu", cfg.mtu},
                        {"dtls", !cfg.disable_dtls},
                        {"extra_args", extra_args},
                        {"log_path", cfg.log_file},
                        {"vpn_engine", cfg.vpn_engine},
                        {"openconnect_runtime", cfg.openconnect_runtime},
                        {"windows_tunnel_driver", cfg.windows_tunnel_driver},
                        {"windows_tap_interface", cfg.windows_tap_interface},
                        {"auto_reconnect", cfg.auto_reconnect},
                        {"minimal_mode", cfg.minimal_mode},
                        {"service_install_prompt_seen",
                         cfg.service_install_prompt_seen},
                        {"minimal_install_service_before_connect",
                         cfg.minimal_install_service_before_connect}};
}

nlohmann::json routes_json(const Config &cfg) {
  nlohmann::json arr = nlohmann::json::array();
  for (const auto &route : cfg.routes) {
    arr.push_back({{"cidr", route}});
  }
  return arr;
}

nlohmann::json key_status_json() {
  const std::string status = config_api::key_status();
  return nlohmann::json{{"present", status == "valid"},
                        {"fingerprint", status == "valid"
                                            ? nlohmann::json("active")
                                            : nlohmann::json(nullptr)},
                        {"status", status}};
}

nlohmann::json service_status_json() {
  return platform::service_status_to_json(platform::current_service_status());
}

nlohmann::json runtime_status_json(const Config &cfg) {
  return platform::runtime_status_json(config::to_platform_config_view(cfg));
}

nlohmann::json frontend_status_from_controller_snapshot(
    const exv::core::TunnelStatusSnapshot &snap, const Config &cfg) {
  const bool running = snap.phase != exv::core::TunnelPhase::Idle &&
                       snap.phase != exv::core::TunnelPhase::Failed;
  const bool connected = running && snap.network_ready;

  nlohmann::json j;
  j["connected"] = connected;
  j["process_running"] = running;
  j["server"] = snap.server.empty() ? cfg.server : snap.server;
  j["username"] = cfg.username;
  j["pid"] = -1;
  j["network_ready"] = snap.network_ready;
  j["interface"] = snap.interface_name;
  j["internal_ip"] = "";
  j["route_count"] = static_cast<int>(cfg.routes.size());
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = 0;
  j["tx_bytes"] = 0;
  if (snap.last_error.has_value()) {
    j["error"] = snap.last_error->message;
    j["error_code"] = snap.last_error->code;
    j["error_recoverable"] = snap.last_error->recoverable;
  }
  if (snap.reconnect.has_value()) {
    j["reconnect_attempt"] = snap.reconnect->attempt;
    j["reconnect_next_retry_ms"] = snap.reconnect->next_retry_ms;
  }
  j["auto_reconnect"] = snap.auto_reconnect;
  j["phase"] = exv::core::tunnel_phase_wire_name(snap.phase);
  try {
    virtual_network::add_status_fields(j, snap.interface_name);
  } catch (...) {
  }
  return j;
}

nlohmann::json helper_current_instance_from_controller_snapshot(
    const exv::core::TunnelStatusSnapshot &snap) {
  nlohmann::json j;
  j["mode"] = snap.helper_mode;
  j["status"] = snap.helper_status;
  j["lease_active"] = snap.core_lease_active;
  j["session_active"] = snap.session_active;
  j["endpoint"] = snap.helper_endpoint;
  j["phase"] = exv::core::tunnel_phase_wire_name(snap.phase);
  return j;
}

nlohmann::json driver_status_json(const Config &cfg) {
  return platform::driver_status_json(config::to_platform_config_view(cfg));
}

nlohmann::json install_driver(const Config &cfg, const nlohmann::json &payload) {
  return platform::install_driver(config::to_platform_config_view(cfg), payload);
}

} // namespace app_api
} // namespace ecnuvpn
