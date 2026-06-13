// ── Import ──────────────────────────────────────────────────────

Config import_from(const std::string &path) {
  if (!utils::file_exists(path)) {
    utils::print_error("Import file not found: " + path);
    return load();
  }
  try {
    std::string content = utils::read_file(path);
    auto j = nlohmann::json::parse(content);
    Config cfg = load();

    if (j.contains("server"))
      cfg.server = j["server"].get<std::string>();
    if (j.contains("username"))
      cfg.username = j["username"].get<std::string>();
    if (j.contains("mtu"))
      cfg.mtu = j["mtu"].get<int>();
    if (j.contains("useragent"))
      cfg.useragent = j["useragent"].get<std::string>();
    if (j.contains("disable_dtls"))
      cfg.disable_dtls = j["disable_dtls"].get<bool>();
    if (j.contains("routes"))
      cfg.routes = j["routes"].get<std::vector<std::string>>();
    if (j.contains("extra_args"))
      cfg.extra_args = j["extra_args"].get<std::vector<std::string>>();
    if (j.contains("log_file"))
      cfg.log_file = j["log_file"].get<std::string>();
    if (j.contains("remember_password"))
      cfg.remember_password = j["remember_password"].get<bool>();
    if (j.contains("vpn_engine"))
      cfg.vpn_engine = j["vpn_engine"].get<std::string>();
    if (j.contains("openconnect_runtime"))
      cfg.openconnect_runtime = j["openconnect_runtime"].get<std::string>();
    if (j.contains("windows_tunnel_driver"))
      cfg.windows_tunnel_driver =
          j["windows_tunnel_driver"].get<std::string>();
    if (j.contains("windows_tap_interface"))
      cfg.windows_tap_interface =
          j["windows_tap_interface"].get<std::string>();
    if (j.contains("auto_reconnect"))
      cfg.auto_reconnect = j["auto_reconnect"].get<bool>();
    if (j.contains("minimal_mode"))
      cfg.minimal_mode = j["minimal_mode"].get<bool>();
    if (j.contains("service_install_prompt_seen"))
      cfg.service_install_prompt_seen =
          j["service_install_prompt_seen"].get<bool>();
    if (j.contains("minimal_install_service_before_connect"))
      cfg.minimal_install_service_before_connect =
          j["minimal_install_service_before_connect"].get<bool>();

    if (j.contains("password")) {
      std::string pw = j["password"].get<std::string>();
      if (!pw.empty() && cfg.remember_password) {
        std::string ks = crypto::key_status();
        if (ks == "valid") {
          cfg.password = crypto::encrypt(pw, crypto::load_key());
          utils::print_info("Password from import file encrypted and stored.");
        } else {
          utils::print_warning("Key is " + ks +
                               " — password from import NOT stored.");
        }
      }
    }

    save(cfg);
    utils::print_success("Config imported from: " + path);
    logger::info("Config imported from: " + path);
    return cfg;
  } catch (const std::exception &e) {
    utils::print_error("Failed to import: " + std::string(e.what()));
    logger::error("Config import error: " + std::string(e.what()));
    return load();
  }
}
