#include "core/use_cases/config_use_cases.hpp"

#include "core/config/config_api.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/runtime_paths.hpp"

#include <algorithm>
#include <sstream>
#include <utility>
#include <vector>

namespace exv::core {
namespace {

using ecnuvpn::Config;

nlohmann::json auth_json(const Config &cfg) {
  return nlohmann::json{{"server", cfg.server},
                        {"username", cfg.username},
                        {"password", ""},
                        {"password_stored", !cfg.password.empty()},
                        {"user_agent", cfg.useragent},
                        {"remember_password", cfg.remember_password}};
}

nlohmann::json settings_json(const Config &cfg) {
  std::string extra_args;
  for (std::size_t i = 0; i < cfg.extra_args.size(); ++i) {
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

nlohmann::json full_config_json(const Config &cfg) {
  nlohmann::json config = cfg;
  config["password"] = "";
  config["password_stored"] = !cfg.password.empty();
  return config;
}

nlohmann::json route_array_json(const Config &cfg) {
  nlohmann::json routes = nlohmann::json::array();
  for (const auto &route : cfg.routes) {
    routes.push_back({{"cidr", route},
                      {"destination", route},
                      {"enabled", true}});
  }
  return routes;
}

const nlohmann::json &settings_payload(const nlohmann::json &payload) {
  if (payload.contains("settings") && payload["settings"].is_object()) {
    return payload["settings"];
  }
  if (payload.contains("config") && payload["config"].is_object()) {
    return payload["config"];
  }
  return payload;
}

std::string bool_value(bool value) { return value ? "true" : "false"; }

UseCaseResult error_from_config_api(const std::string &message) {
  if (message.find("not found") != std::string::npos ||
      message.find("Not found") != std::string::npos) {
    return UseCaseResult::fail("not_found", message);
  }
  if (message.find("already exists") != std::string::npos) {
    return UseCaseResult::fail("already_exists", message);
  }
  return UseCaseResult::fail("invalid_payload", message);
}

std::string route_cidr_from_payload(const nlohmann::json &payload) {
  if (payload.contains("cidr") && payload["cidr"].is_string()) {
    return payload["cidr"].get<std::string>();
  }
  if (payload.contains("destination") && payload["destination"].is_string()) {
    return payload["destination"].get<std::string>();
  }
  return "";
}

} // namespace

ConfigUseCases::ConfigUseCases()
    : ConfigUseCases(ecnuvpn::platform::get_config_dir()) {}

ConfigUseCases::ConfigUseCases(std::string config_dir)
    : manager_(std::move(config_dir)) {
  ecnuvpn::platform::logging::configure_default_logging(false);
}

Config ConfigUseCases::load_config() { return manager_.load(); }

UseCaseResult ConfigUseCases::get_config() {
  Config cfg = manager_.load();
  return UseCaseResult::ok({{"config", full_config_json(cfg)},
                            {"auth", auth_json(cfg)},
                            {"settings", settings_json(cfg)},
                            {"routes", route_array_json(cfg)}});
}

UseCaseResult ConfigUseCases::save_config(const nlohmann::json &payload) {
  UseCaseResult auth = save_auth(payload);
  if (!auth.success) {
    return auth;
  }
  return save_settings(payload);
}

UseCaseResult ConfigUseCases::get_profile(const nlohmann::json &payload) {
  if (!payload.contains("profile_id") || !payload["profile_id"].is_string()) {
    return UseCaseResult::fail("invalid_payload", "profile_id is required");
  }
  return UseCaseResult::fail(
      "unsupported_action",
      "Named config profiles are not supported by the current config model.");
}

UseCaseResult ConfigUseCases::save_profile(const nlohmann::json &payload) {
  if (!payload.contains("profile_id") || !payload["profile_id"].is_string()) {
    return UseCaseResult::fail("invalid_payload", "profile_id is required");
  }
  return UseCaseResult::fail(
      "unsupported_action",
      "Named config profiles are not supported by the current config model.");
}

UseCaseResult ConfigUseCases::get_auth() {
  return UseCaseResult::ok(auth_json(manager_.load()));
}

UseCaseResult ConfigUseCases::save_auth(const nlohmann::json &payload) {
  if (payload.contains("server") && payload["server"].is_string()) {
    std::string err = ecnuvpn::config_api::config_set(
        manager_, "server", payload["server"].get<std::string>());
    if (!err.empty()) {
      return error_from_config_api(err);
    }
  }
  if (payload.contains("username") && payload["username"].is_string()) {
    std::string err = ecnuvpn::config_api::config_set(
        manager_, "username", payload["username"].get<std::string>());
    if (!err.empty()) {
      return error_from_config_api(err);
    }
  }
  if (payload.contains("user_agent") && payload["user_agent"].is_string()) {
    const std::string user_agent = payload["user_agent"].get<std::string>();
    if (!user_agent.empty()) {
      std::string err =
          ecnuvpn::config_api::config_set(manager_, "useragent", user_agent);
      if (!err.empty()) {
        return error_from_config_api(err);
      }
    }
  }
  const bool has_submitted_password =
      payload.contains("password") && payload["password"].is_string() &&
      !payload["password"].get<std::string>().empty();
  if (payload.contains("remember_password") &&
      payload["remember_password"].is_boolean()) {
    const bool remember = payload["remember_password"].get<bool>();
    if (!remember) {
      std::string err =
          ecnuvpn::config_api::config_clear_password_and_key(manager_);
      if (!err.empty()) {
        return error_from_config_api(err);
      }
    } else {
      Config current = manager_.load();
      if (!has_submitted_password && current.password.empty()) {
        return UseCaseResult::fail(
            "invalid_payload",
            "Password is required to enable remember_password.");
      }
      std::string err = ecnuvpn::config_api::config_set(
          manager_, "remember_password", "true");
      if (!err.empty()) {
        return error_from_config_api(err);
      }
    }
  }
  if (has_submitted_password) {
    std::string err = ecnuvpn::config_api::config_set_password(
        manager_, payload["password"].get<std::string>());
    if (!err.empty()) {
      return error_from_config_api(err);
    }
  }
  return get_auth();
}

UseCaseResult ConfigUseCases::get_settings() {
  return UseCaseResult::ok(settings_json(manager_.load()));
}

UseCaseResult ConfigUseCases::save_settings(const nlohmann::json &payload) {
  const nlohmann::json &settings = settings_payload(payload);

  auto set_string = [&](const char *json_key, const char *config_key)
      -> UseCaseResult {
    if (settings.contains(json_key) && settings[json_key].is_string()) {
      std::string err = ecnuvpn::config_api::config_set(
          manager_, config_key, settings[json_key].get<std::string>());
      if (!err.empty()) {
        return error_from_config_api(err);
      }
    }
    return UseCaseResult::ok();
  };

  auto set_bool = [&](const char *json_key, const char *config_key)
      -> UseCaseResult {
    if (settings.contains(json_key) && settings[json_key].is_boolean()) {
      std::string err = ecnuvpn::config_api::config_set(
          manager_, config_key, bool_value(settings[json_key].get<bool>()));
      if (!err.empty()) {
        return error_from_config_api(err);
      }
    }
    return UseCaseResult::ok();
  };

  if (settings.contains("mtu") && settings["mtu"].is_number_integer()) {
    std::string err = ecnuvpn::config_api::config_set(
        manager_, "mtu", std::to_string(settings["mtu"].get<int>()));
    if (!err.empty()) {
      return error_from_config_api(err);
    }
  }
  if (settings.contains("dtls") && settings["dtls"].is_boolean()) {
    std::string err = ecnuvpn::config_api::config_set(
        manager_, "disable_dtls", settings["dtls"].get<bool>() ? "false"
                                                               : "true");
    if (!err.empty()) {
      return error_from_config_api(err);
    }
  }
  if (settings.contains("disable_dtls") &&
      settings["disable_dtls"].is_boolean()) {
    std::string err = ecnuvpn::config_api::config_set(
        manager_, "disable_dtls",
        bool_value(settings["disable_dtls"].get<bool>()));
    if (!err.empty()) {
      return error_from_config_api(err);
    }
  }
  if (settings.contains("extra_args")) {
    Config updated = manager_.load();
    if (settings["extra_args"].is_array()) {
      updated.extra_args = settings["extra_args"].get<std::vector<std::string>>();
    } else if (settings["extra_args"].is_string()) {
      const std::string value = settings["extra_args"].get<std::string>();
      updated.extra_args =
          value.empty() ? std::vector<std::string>{}
                        : std::vector<std::string>{value};
    } else {
      return UseCaseResult::fail("invalid_payload",
                                 "extra_args must be a string or array");
    }
    if (!manager_.save(updated)) {
      return UseCaseResult::fail("config_save_failed",
                                 "Failed to write config file.");
    }
  }

  for (UseCaseResult result :
       {set_string("log_path", "log_file"),
        set_string("log_file", "log_file"),
        set_string("vpn_engine", "vpn_engine"),
        set_string("openconnect_runtime", "openconnect_runtime"),
        set_string("windows_tunnel_driver", "windows_tunnel_driver"),
        set_string("windows_tap_interface", "windows_tap_interface"),
        set_bool("auto_reconnect", "auto_reconnect"),
        set_bool("minimal_mode", "minimal_mode"),
        set_bool("service_install_prompt_seen",
                 "service_install_prompt_seen"),
        set_bool("minimal_install_service_before_connect",
                 "minimal_install_service_before_connect")}) {
    if (!result.success) {
      return result;
    }
  }

  Config cfg = manager_.load();
  return UseCaseResult::ok({{"saved", true},
                            {"config", full_config_json(cfg)},
                            {"settings", settings_json(cfg)}});
}

UseCaseResult ConfigUseCases::get_key_status() {
  const std::string status = ecnuvpn::config_api::key_status();
  return UseCaseResult::ok(
      {{"present", status == "valid"},
       {"fingerprint", status == "valid" ? nlohmann::json("active")
                                         : nlohmann::json(nullptr)},
       {"status", status}});
}

UseCaseResult ConfigUseCases::list_routes() {
  return UseCaseResult::ok({{"routes", route_array_json(manager_.load())}});
}

UseCaseResult ConfigUseCases::add_route(const nlohmann::json &payload) {
  const std::string cidr = route_cidr_from_payload(payload);
  if (cidr.empty()) {
    return UseCaseResult::fail("invalid_payload",
                               "cidr or destination is required");
  }
  std::string err = ecnuvpn::config_api::route_add(manager_, cidr);
  if (!err.empty()) {
    return error_from_config_api(err);
  }
  return UseCaseResult::ok(
      {{"added", true}, {"routes", route_array_json(manager_.load())}});
}

UseCaseResult ConfigUseCases::remove_route(const nlohmann::json &payload) {
  const std::string cidr = route_cidr_from_payload(payload);
  if (cidr.empty()) {
    return UseCaseResult::fail("invalid_payload",
                               "cidr or destination is required");
  }
  std::string err = ecnuvpn::config_api::route_remove(manager_, cidr);
  if (!err.empty()) {
    return error_from_config_api(err);
  }
  return UseCaseResult::ok(
      {{"removed", true}, {"routes", route_array_json(manager_.load())}});
}

UseCaseResult ConfigUseCases::reset_routes() {
  ecnuvpn::config_api::route_reset_defaults(manager_);
  return UseCaseResult::ok(
      {{"reset", true}, {"routes", route_array_json(manager_.load())}});
}

UseCaseResult ConfigUseCases::route_enable_unsupported() {
  return UseCaseResult::fail(
      "unsupported_action",
      "Persisted route enablement is not supported by the config model.");
}

UseCaseResult ConfigUseCases::route_disable_unsupported() {
  return UseCaseResult::fail(
      "unsupported_action",
      "Persisted route disablement is not supported by the config model.");
}

} // namespace exv::core
