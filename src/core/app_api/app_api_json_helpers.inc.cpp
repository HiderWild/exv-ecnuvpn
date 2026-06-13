nlohmann::json error(const std::string &message,
                     const std::string &code = std::string()) {
  // Route through the unified feedback module: guarantees a canonical,
  // non-empty code plus recoverable/recommended_action. The frontend contract
  // uses the "error" key for the human message.
  feedback::ErrorInfo info = feedback::lookup_error(code, message);
  return nlohmann::json{{"ok", false},
                        {"error", message},
                        {"code", info.code},
                        {"recoverable", info.recoverable},
                        {"recommended_action", info.recommended_action}};
}

std::string json_string(const nlohmann::json &object, const char *key,
                        const std::string &fallback = std::string());

bool helper_unavailable(const nlohmann::json &response) {
  return json_string(response, "code") == platform::kHelperUnavailableCode ||
         json_string(response, "message") == "Helper daemon not available";
}

using StageTimer = exv::core::ConnectStageTimer;

bool json_bool(const nlohmann::json &object, const char *key, bool fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null())
    return fallback;
  if (object[key].is_boolean())
    return object[key].get<bool>();
  return fallback;
}

int json_int(const nlohmann::json &object, const char *key, int fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null())
    return fallback;
  if (object[key].is_number_integer())
    return object[key].get<int>();
  return fallback;
}

uint64_t json_u64(const nlohmann::json &object, const char *key,
                  uint64_t fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null())
    return fallback;
  if (object[key].is_number_unsigned())
    return object[key].get<uint64_t>();
  if (object[key].is_number_integer()) {
    int64_t value = object[key].get<int64_t>();
    return value < 0 ? fallback : static_cast<uint64_t>(value);
  }
  return fallback;
}

std::string json_string(const nlohmann::json &object, const char *key,
                        const std::string &fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null())
    return fallback;
  if (object[key].is_string())
    return object[key].get<std::string>();
  return fallback;
}

nlohmann::json helper_error(const nlohmann::json &response,
                            const std::string &fallback_message) {
  return error(json_string(response, "message", fallback_message),
               json_string(response, "code"));
}

config::ConfigManager make_config_manager() {
  utils::ensure_dir(utils::get_config_dir());
  logger::init();
  return config::ConfigManager(utils::get_config_dir());
}

void apply_desktop_runtime_context(const nlohmann::json &payload) {
  if (!payload.is_object())
    return;

  std::string home = json_string(payload, "home");
  std::string config_dir = json_string(payload, "config_dir");
  if (home.empty() && config_dir.empty())
    return;

  utils::set_runtime_path_override(home.empty() ? utils::get_effective_home()
                                                : home,
                                   config_dir);
#ifndef _WIN32
  std::string owner_home = home.empty() ? utils::get_effective_home() : home;
  struct stat home_stat {};
  if (!owner_home.empty() && stat(owner_home.c_str(), &home_stat) == 0) {
    utils::set_runtime_owner(home_stat.st_uid, home_stat.st_gid);
  }
#endif
}

void add_desktop_owner_context(nlohmann::json &request) {
#ifndef _WIN32
  if (!utils::has_runtime_owner())
    return;
  request["owner_uid"] =
      static_cast<unsigned int>(utils::get_runtime_owner_uid());
  request["owner_gid"] =
      static_cast<unsigned int>(utils::get_runtime_owner_gid());
#else
  (void)request;
#endif
}

nlohmann::json frontend_status_from_helper(const nlohmann::json &helper_resp,
                                           const Config &cfg) {
  bool running = json_bool(helper_resp, "running", false);
  bool network_ready = json_bool(helper_resp, "network_ready", false);
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

nlohmann::json frontend_status_from_snapshot_json(const nlohmann::json &snapshot,
                                                   const Config &cfg) {
  std::string iface = json_string(snapshot, "interface");
  bool running = json_bool(snapshot, "running", false);
  bool network_ready = json_bool(snapshot, "network_ready", false);
  uint64_t rx_bytes = 0;
  uint64_t tx_bytes = 0;
  if (network_ready && !iface.empty()) {
    utils::get_interface_traffic(iface, &rx_bytes, &tx_bytes);
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

std::string helper_binary_next_to_exv() {
  std::filesystem::path exv_path(utils::get_executable_path());
#ifdef _WIN32
  return (exv_path.parent_path() / "exv-helper.exe").string();
#else
  return (exv_path.parent_path() / "exv-helper").string();
#endif
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
