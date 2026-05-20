#include "app_api.hpp"

#include "config.hpp"
#include "config_api.hpp"
#include "config_manager.hpp"
#include "crypto.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"
#include "utils.hpp"
#include "virtual_network.hpp"

#include <algorithm>
#include <cstdio>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace app_api {
namespace {

nlohmann::json error(const std::string &message,
                     const std::string &code = std::string()) {
  nlohmann::json result{{"ok", false}, {"error", message}};
  if (!code.empty())
    result["code"] = code;
  return result;
}

bool helper_unavailable(const nlohmann::json &response) {
  return response.value("code", std::string()) ==
             platform::kHelperUnavailableCode ||
         response.value("message", std::string()) ==
             "Helper daemon not available";
}

nlohmann::json helper_error(const nlohmann::json &response,
                            const std::string &fallback_message) {
  return error(response.value("message", fallback_message),
               response.value("code", std::string()));
}

config::ConfigManager make_config_manager() {
  utils::ensure_dir(utils::get_config_dir());
  // The desktop / RPC entrypoint does not go through the CLI wizard, so we
  // have to make sure the encryption key file exists before any password
  // operation. Without this, config_set_password would always fail with
  // "Encryption key is missing" on a fresh install.
  crypto::init_key_if_needed();
  logger::init();
  return config::ConfigManager(utils::get_config_dir());
}

nlohmann::json frontend_status_from_helper(const nlohmann::json &helper_resp,
                                           const Config &cfg) {
  nlohmann::json j;
  j["connected"] = helper_resp.value("running", false);
  j["server"] = helper_resp.value("server", cfg.server);
  j["username"] = cfg.username;
  j["pid"] = helper_resp.value("pid", -1);
  j["supervisor_pid"] = helper_resp.value("supervisor_pid", -1);
  j["network_ready"] = helper_resp.value("network_ready", false);
  j["interface"] = helper_resp.value("interface", "");
  j["internal_ip"] = helper_resp.value("internal_ip", "");
  j["route_count"] = helper_resp.value("route_count",
                                        static_cast<int>(cfg.routes.size()));
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = helper_resp.value("rx_bytes", 0);
  j["tx_bytes"] = helper_resp.value("tx_bytes", 0);
  virtual_network::add_status_fields(j, j.value("interface", std::string()));
  return j;
}

nlohmann::json disconnected_status(const Config &cfg) {
  return frontend_status_from_helper(nlohmann::json{{"running", false}}, cfg);
}

nlohmann::json frontend_status_from_snapshot_json(const nlohmann::json &snapshot,
                                                   const Config &cfg) {
  std::string iface = snapshot.value("interface", std::string());
  uint64_t rx_bytes = 0;
  uint64_t tx_bytes = 0;
  if (snapshot.value("network_ready", false) && !iface.empty()) {
    utils::get_interface_traffic(iface, &rx_bytes, &tx_bytes);
  }

  nlohmann::json j;
  j["connected"] = snapshot.value("running", false);
  j["server"] = cfg.server;
  j["username"] = cfg.username;
  j["pid"] = snapshot.value("pid", -1);
  j["supervisor_pid"] = snapshot.value("supervisor_pid", -1);
  j["network_ready"] = snapshot.value("network_ready", false);
  j["interface"] = iface;
  j["internal_ip"] = snapshot.value("internal_ip", std::string());
  j["route_count"] = static_cast<int>(cfg.routes.size());
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = rx_bytes;
  j["tx_bytes"] = tx_bytes;
  virtual_network::add_status_fields(j, iface);
  return j;
}

nlohmann::json auth_config(const Config &cfg) {
  // Never echo the stored ciphertext or a fake mask back to the UI. The UI
  // shows a placeholder when password_stored is true and treats an empty
  // submitted password as "keep the existing one".
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
    if (i > 0)
      extra_args += " ";
    extra_args += cfg.extra_args[i];
  }

  return nlohmann::json{{"mtu", cfg.mtu},
                        {"dtls", !cfg.disable_dtls},
                        {"extra_args", extra_args},
                        {"log_path", cfg.log_file},
                        {"webui_port", cfg.webui_port},
                        {"webui_host", cfg.webui_bind},
                        {"webui_enabled", cfg.webui_enabled},
                        {"openconnect_runtime", cfg.openconnect_runtime},
                        {"windows_tunnel_driver", cfg.windows_tunnel_driver},
                        {"windows_tap_interface", cfg.windows_tap_interface}};
}

nlohmann::json routes_json(const Config &cfg) {
  nlohmann::json arr = nlohmann::json::array();
  for (const auto &route : cfg.routes) {
    arr.push_back({{"cidr", route}});
  }
  return arr;
}

nlohmann::json key_status_json() {
  std::string status = config_api::key_status();
  return nlohmann::json{{"present", status == "valid"},
                        {"fingerprint", status == "valid"
                                            ? nlohmann::json("active")
                                            : nlohmann::json(nullptr)},
                        {"status", status}};
}

nlohmann::json service_status_json() {
  return platform::service_status_to_json(platform::current_service_status());
}

std::string json_safe_text(const std::string &text) {
  std::string out;
  out.reserve(text.size());
  for (unsigned char c : text) {
    if (c == '\t' || c == '\n' || c == '\r' || (c >= 0x20 && c < 0x80)) {
      out.push_back(static_cast<char>(c));
    } else {
      out.push_back('?');
    }
  }
  return out;
}

nlohmann::json runtime_status_json(const Config &cfg) {
  return platform::runtime_status_json(cfg);
}

nlohmann::json driver_status_json(const Config &cfg) {
  return platform::driver_status_json(cfg);
}

nlohmann::json install_driver(const Config &cfg, const nlohmann::json &payload) {
  return platform::install_driver(cfg, payload);
}

nlohmann::json preflight_connect(const Config &cfg, const std::string &password,
                                bool allow_direct_fallback = false) {
  if (cfg.server.empty())
    return error("VPN server is not configured.");
  if (cfg.username.empty())
    return error("VPN username is not configured.");
  if (password.empty())
    return error("VPN password is not configured.");

  if (!helper::is_available() && !allow_direct_fallback) {
    return error(platform::helper_unavailable_connect_message(),
                 platform::kHelperUnavailableCode);
  }

  nlohmann::json runtime = runtime_status_json(cfg);
  if (!runtime.value("available", false)) {
    return error("OpenConnect runtime is not available. The desktop bundle is missing openconnect and its native dependencies.");
  }

  nlohmann::json platform_err = platform::preflight_connect_platform_checks(cfg);
  if (platform_err.is_object() && platform_err.value("ok", true) == false)
    return platform_err;

  return nlohmann::json{{"ok", true}};
}

nlohmann::json logs_json(const nlohmann::json &payload) {
  config::ConfigManager mgr = make_config_manager();
  Config cfg = mgr.load();
  std::string log_path = utils::expand_home(cfg.log_file);
  int max_lines = payload.value("lines", 100);
  if (max_lines < 1)
    max_lines = 1;
  if (max_lines > 10000)
    max_lines = 10000;
  std::string filter = payload.value("filter", std::string());

  nlohmann::json lines = nlohmann::json::array();
  std::vector<std::string> all_lines;
  std::ifstream ifs(log_path);
  std::string line;
  while (std::getline(ifs, line)) {
    if (!line.empty() && line.back() == '\r')
      line.pop_back();
    if (filter.empty() || line.find(filter) != std::string::npos)
      all_lines.push_back(line);
  }

  size_t start = all_lines.size() > static_cast<size_t>(max_lines)
                     ? all_lines.size() - static_cast<size_t>(max_lines)
                     : 0;
  for (size_t i = start; i < all_lines.size(); ++i) {
    lines.push_back({{"timestamp", ""},
                     {"level", "info"},
                     {"message", json_safe_text(all_lines[i])}});
  }
  return lines;
}

} // namespace

nlohmann::json handle_action(const std::string &action,
                             const nlohmann::json &payload) {
  try {
    config::ConfigManager mgr = make_config_manager();
    Config cfg = mgr.load();

    if (action == "status.get") {
      auto helper_resp = platform::send_helper_request({{"action", "status"}});
      if (!helper_resp.value("ok", false) && helper_unavailable(helper_resp)) {
        nlohmann::json fallback = platform::status_fallback_without_helper(cfg);
        if (fallback.value("_snapshot", false)) {
          if (fallback.value("_running", false)) {
            return frontend_status_from_snapshot_json(
                fallback.value("_snapshot_data", nlohmann::json::object()), cfg);
          }
          return disconnected_status(cfg);
        }
      }
      return frontend_status_from_helper(helper_resp, cfg);
    }

    if (action == "vpn.connect") {
      std::string password = payload.value("password", std::string());
      bool allow_direct_fallback =
          payload.value("allow_direct_fallback", false);
      if (password.empty() && !cfg.password.empty()) {
        std::string key = crypto::load_key();
        if (!key.empty())
          password = crypto::decrypt(cfg.password, key);
      }
      nlohmann::json preflight =
          preflight_connect(cfg, password, allow_direct_fallback);
      if (preflight.is_object() && preflight.value("ok", true) == false)
        return preflight;

      if (!helper::is_available() && allow_direct_fallback) {
        nlohmann::json direct =
            platform::try_connect_direct_fallback(cfg, password);
        if (direct.is_object() && direct.value("_direct_fallback", false)) {
          auto &sd = direct["_snapshot_data"];
          if (sd.value("running", false))
            return frontend_status_from_snapshot_json(sd, cfg);
          return disconnected_status(cfg);
        }
        if (direct.is_object() && direct.value("ok", true) == false)
          return direct;
      }

        auto helper_resp = platform::send_helper_request(
          {{"action", "start"},
           {"config", cfg},
           {"password", password},
           {"retry_limit", 0},
           {"home", utils::get_effective_home()},
           {"config_dir", utils::get_config_dir()}});
      if (!helper_resp.value("ok", false)) {
        return helper_error(helper_resp, "Failed to start VPN");
      }
      return frontend_status_from_helper(helper_resp, cfg);
    }

    if (action == "vpn.disconnect") {
      bool allow_direct_fallback =
          payload.value("allow_direct_fallback", false);
      auto helper_resp = platform::send_helper_request({{"action", "stop"}});
      if (!helper_resp.value("ok", false) && helper_unavailable(helper_resp)) {
        nlohmann::json direct =
            platform::try_disconnect_direct_fallback(allow_direct_fallback);
        if (direct.is_object() && direct.value("_not_running", false))
          return disconnected_status(cfg);
        if (direct.is_object() && direct.value("_direct_fallback", false))
          return disconnected_status(cfg);
        if (!allow_direct_fallback) {
          return error(platform::helper_unavailable_disconnect_message(),
                       platform::kHelperUnavailableCode);
        }
        if (direct.is_object() && direct.value("ok", true) == false)
          return direct;
        return disconnected_status(cfg);
      }
      if (!helper_resp.value("ok", false)) {
        return helper_error(helper_resp, "Failed to stop VPN");
      }
      return disconnected_status(cfg);
    }

    if (action == "config.getAuth")
      return auth_config(cfg);

    if (action == "config.saveAuth") {
      if (payload.contains("server") && payload["server"].is_string()) {
        std::string err = config_api::config_set(mgr, "server", payload["server"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("username") && payload["username"].is_string()) {
        std::string err = config_api::config_set(mgr, "username", payload["username"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      // Update remember_password BEFORE password so that the password setter
      // sees the correct toggle state. The UI treats an empty password field
      // as "keep the existing password" while a non-empty value means update.
      if (payload.contains("remember_password") &&
          payload["remember_password"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "remember_password",
                               payload["remember_password"].get<bool>() ? "true"
                                                                         : "false");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("password") && payload["password"].is_string()) {
        std::string password = payload["password"].get<std::string>();
        if (!password.empty()) {
          std::string err = config_api::config_set_password(mgr, password);
          if (!err.empty())
            return error(err);
        }
      }
      if (payload.contains("user_agent") && payload["user_agent"].is_string()) {
        std::string value = payload["user_agent"].get<std::string>();
        // Treat empty / whitespace user_agent as "no change" so the UI cannot
        // accidentally overwrite the platform default.
        if (!utils::trim(value).empty()) {
          std::string err = config_api::config_set(mgr, "useragent", value);
          if (!err.empty()) return error(err);
        }
      }
      return auth_config(mgr.load());
    }

    if (action == "config.getSettings")
      return settings_config(cfg);

    if (action == "config.saveSettings") {
      if (payload.contains("mtu") && payload["mtu"].is_number_integer()) {
        std::string err = config_api::config_set(mgr, "mtu", std::to_string(payload["mtu"].get<int>()));
        if (!err.empty()) return error(err);
      }
      if (payload.contains("dtls") && payload["dtls"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "disable_dtls",
                               payload["dtls"].get<bool>() ? "false" : "true");
     if (!err.empty()) return error(err);
      }
      if (payload.contains("extra_args") && payload["extra_args"].is_string()) {
        Config updated = mgr.load();
        std::string value = payload["extra_args"].get<std::string>();
        updated.extra_args = value.empty() ? std::vector<std::string>{}
                                           : std::vector<std::string>{value};
        mgr.save(updated);
      }
      if (payload.contains("log_path") && payload["log_path"].is_string()) {
        std::string err = config_api::config_set(mgr, "log_file", payload["log_path"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("webui_port") && payload["webui_port"].is_number_integer()) {
        Config updated = mgr.load();
        updated.webui_port = payload["webui_port"].get<int>();
        mgr.save(updated);
      }
      if (payload.contains("webui_host") && payload["webui_host"].is_string()) {
        Config updated = mgr.load();
        updated.webui_bind = payload["webui_host"].get<std::string>();
        mgr.save(updated);
      }
      if (payload.contains("webui_enabled") && payload["webui_enabled"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "webui_enabled",
                               payload["webui_enabled"].get<bool>() ? "true" : "false");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("openconnect_runtime") &&
          payload["openconnect_runtime"].is_string()) {
        std::string err = config_api::config_set(mgr, "openconnect_runtime",
                               payload["openconnect_runtime"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("windows_tunnel_driver") &&
          payload["windows_tunnel_driver"].is_string()) {
        std::string err = config_api::config_set(mgr, "windows_tunnel_driver",
                               payload["windows_tunnel_driver"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("windows_tap_interface") &&
          payload["windows_tap_interface"].is_string()) {
        std::string err = config_api::config_set(mgr, "windows_tap_interface",
                               payload["windows_tap_interface"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      return settings_config(mgr.load());
    }

    if (action == "config.getKey")
      return key_status_json();

    if (action == "routes.list")
      return routes_json(cfg);

    if (action == "routes.add") {
      std::string err = config_api::route_add(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return error(err);
      return routes_json(mgr.load());
    }

    if (action == "routes.remove") {
      std::string err = config_api::route_remove(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return error(err);
      return routes_json(mgr.load());
    }

    if (action == "routes.reset") {
      config_api::route_reset_defaults(mgr);
      return routes_json(mgr.load());
    }

    if (action == "service.status")
      return service_status_json();

    if (action == "helper.status")
      return service_status_json();

    if (action == "runtime.status")
      return runtime_status_json(cfg);

    if (action == "drivers.status")
      return driver_status_json(cfg);

    if (action == "drivers.install")
      return install_driver(cfg, payload);

    if (action == "logs.list")
      return logs_json(payload);

    return error("Unknown desktop action: " + action);
  } catch (const std::exception &ex) {
    return error(ex.what());
  } catch (...) {
    return error("Unknown desktop API error");
  }
}

} // namespace app_api
} // namespace ecnuvpn