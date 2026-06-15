#include "core/app_api/desktop_config_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/app_api/desktop_status_presenter.hpp"
#include "core/config/config_api.hpp"
#include "core/config/config_manager.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "common/diagnostics/logger.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/runtime_paths.hpp"
#include "utils/strings.hpp"

#include <string>
#include <vector>

namespace ecnuvpn {
namespace app_api {
namespace {

config::ConfigManager make_config_manager() {
  platform::ensure_dir(platform::get_config_dir());
  logger::init();
  return config::ConfigManager(platform::get_config_dir());
}

} // namespace

void register_desktop_config_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "config.getAuth", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        return auth_config(cfg);
      });

  adapter.register_legacy_handler(
      "config.saveAuth", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        if (payload.contains("server") && payload["server"].is_string()) {
          std::string err = config_api::config_set(
              mgr, "server", payload["server"].get<std::string>());
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("username") && payload["username"].is_string()) {
          std::string err = config_api::config_set(
              mgr, "username", payload["username"].get<std::string>());
          if (!err.empty()) {
            return error(err);
          }
        }
        const bool remember_payload =
            payload.contains("remember_password") &&
            payload["remember_password"].is_boolean();
        const bool remember_password =
            remember_payload ? payload["remember_password"].get<bool>()
                             : mgr.load().remember_password;
        const std::string submitted_password =
            payload.contains("password") && payload["password"].is_string()
                ? payload["password"].get<std::string>()
                : std::string();

        if (remember_payload && !remember_password) {
          std::string err = config_api::config_clear_password_and_key(mgr);
          if (!err.empty()) {
            return error(err);
          }
        } else if (remember_password) {
          if (!submitted_password.empty()) {
            std::string err =
                config_api::config_set_password(mgr, submitted_password);
            if (!err.empty()) {
              return error(err);
            }
          } else {
            Config current = mgr.load();
            if (remember_payload && current.password.empty()) {
              return error("Password is required to enable remember_password.");
            }
            std::string err =
                config_api::config_set(mgr, "remember_password", "true");
            if (!err.empty()) {
              return error(err);
            }
          }
        }
        if (payload.contains("user_agent") &&
            payload["user_agent"].is_string()) {
          std::string value = payload["user_agent"].get<std::string>();
          if (!exv::utils::trim(value).empty()) {
            std::string err = config_api::config_set(mgr, "useragent", value);
            if (!err.empty()) {
              return error(err);
            }
          }
        }
        return auth_config(mgr.load());
      });

  adapter.register_legacy_handler(
      "config.getSettings",
      [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        return settings_config(cfg);
      });

  adapter.register_legacy_handler(
      "config.saveSettings",
      [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        if (payload.contains("mtu") && payload["mtu"].is_number_integer()) {
          std::string err = config_api::config_set(
              mgr, "mtu", std::to_string(payload["mtu"].get<int>()));
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("dtls") && payload["dtls"].is_boolean()) {
          std::string err = config_api::config_set(
              mgr, "disable_dtls",
              payload["dtls"].get<bool>() ? "false" : "true");
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("extra_args") &&
            payload["extra_args"].is_string()) {
          Config updated = mgr.load();
          std::string value = payload["extra_args"].get<std::string>();
          updated.extra_args = value.empty() ? std::vector<std::string>{}
                                             : std::vector<std::string>{value};
          mgr.save(updated);
        }
        if (payload.contains("log_path") && payload["log_path"].is_string()) {
          std::string err = config_api::config_set(
              mgr, "log_file", payload["log_path"].get<std::string>());
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("vpn_engine") &&
            payload["vpn_engine"].is_string()) {
          std::string err = config_api::config_set(
              mgr, "vpn_engine", payload["vpn_engine"].get<std::string>());
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("auto_reconnect") &&
            payload["auto_reconnect"].is_boolean()) {
          std::string err = config_api::config_set(
              mgr, "auto_reconnect",
              payload["auto_reconnect"].get<bool>() ? "true" : "false");
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("minimal_mode") &&
            payload["minimal_mode"].is_boolean()) {
          std::string err = config_api::config_set(
              mgr, "minimal_mode",
              payload["minimal_mode"].get<bool>() ? "true" : "false");
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("service_install_prompt_seen") &&
            payload["service_install_prompt_seen"].is_boolean()) {
          std::string err = config_api::config_set(
              mgr, "service_install_prompt_seen",
              payload["service_install_prompt_seen"].get<bool>() ? "true"
                                                                 : "false");
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("minimal_install_service_before_connect") &&
            payload["minimal_install_service_before_connect"].is_boolean()) {
          std::string err = config_api::config_set(
              mgr, "minimal_install_service_before_connect",
              payload["minimal_install_service_before_connect"].get<bool>()
                  ? "true"
                  : "false");
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("openconnect_runtime") &&
            payload["openconnect_runtime"].is_string()) {
          std::string err = config_api::config_set(
              mgr, "openconnect_runtime",
              payload["openconnect_runtime"].get<std::string>());
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("windows_tunnel_driver") &&
            payload["windows_tunnel_driver"].is_string()) {
          std::string err = config_api::config_set(
              mgr, "windows_tunnel_driver",
              payload["windows_tunnel_driver"].get<std::string>());
          if (!err.empty()) {
            return error(err);
          }
        }
        if (payload.contains("windows_tap_interface") &&
            payload["windows_tap_interface"].is_string()) {
          std::string err = config_api::config_set(
              mgr, "windows_tap_interface",
              payload["windows_tap_interface"].get<std::string>());
          if (!err.empty()) {
            return error(err);
          }
        }
        return settings_config(mgr.load());
      });

  adapter.register_legacy_handler(
      "config.getKey", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return key_status_json();
      });
}

} // namespace app_api
} // namespace ecnuvpn
