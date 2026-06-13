// ── Show ────────────────────────────────────────────────────────

void show(const Config &cfg) {
  utils::print_header("EXV Configuration");

  auto pw_status = [&]() -> std::string {
    if (!cfg.remember_password)
      return std::string(utils::DIM) + "(prompt at connect)" + utils::RESET;
    if (cfg.password.empty())
      return "(not set — run: exv config set password)";
    std::string ks = crypto::key_status();
    if (ks == "valid")
      return std::string(utils::GREEN) + "stored (encrypted)" + utils::RESET;
    return std::string(utils::RED) + "[KEY " + ks +
           " — run: exv config key reset]" + utils::RESET;
  };

  std::cout << utils::BOLD << "  Server          : " << utils::RESET
            << cfg.server << std::endl;
  std::cout << utils::BOLD << "  Username        : " << utils::RESET
            << (cfg.username.empty() ? "(not set)" : cfg.username) << std::endl;
  std::cout << utils::BOLD << "  Password        : " << utils::RESET
            << pw_status() << std::endl;
  std::cout << utils::BOLD << "  Remember Passwd : " << utils::RESET
            << (cfg.remember_password
                    ? std::string(utils::GREEN) + "yes" + utils::RESET
                    : std::string(utils::YELLOW) + "no (prompt on connect)" +
                          utils::RESET)
            << std::endl;
  std::cout << utils::BOLD << "  MTU             : " << utils::RESET << cfg.mtu
            << std::endl;
  std::cout << utils::BOLD << "  UserAgent       : " << utils::RESET
            << cfg.useragent << std::endl;
  std::cout << utils::BOLD << "  Disable DTLS    : " << utils::RESET
            << (cfg.disable_dtls
                    ? std::string(utils::YELLOW) + "yes (TLS-only transport)" +
                          utils::RESET
                    : std::string(utils::GREEN) + "no" + utils::RESET)
            << std::endl;
  std::cout << utils::BOLD << "  Log File        : " << utils::RESET
            << cfg.log_file << std::endl;
  std::cout << utils::BOLD << "  Auto Reconnect  : " << utils::RESET
            << (cfg.auto_reconnect ? "true" : "false") << std::endl;
  std::cout << utils::BOLD << "  Minimal Mode    : " << utils::RESET
            << (cfg.minimal_mode ? "true" : "false") << std::endl;
  std::cout << utils::BOLD << "  Minimal Install Service: " << utils::RESET
            << (cfg.minimal_install_service_before_connect ? "true" : "false")
            << std::endl;
  std::cout << utils::BOLD << "  VPN Engine      : " << utils::RESET
            << cfg.vpn_engine << std::endl;
  std::cout << utils::BOLD << "  OpenConnect Runtime: " << utils::RESET
            << cfg.openconnect_runtime << std::endl;
#ifdef _WIN32
  std::cout << utils::BOLD << "  Tunnel Driver   : " << utils::RESET
            << cfg.windows_tunnel_driver << std::endl;
  std::cout << utils::BOLD << "  TAP Interface   : " << utils::RESET
            << (cfg.windows_tap_interface.empty() ? "(auto)"
                                                  : cfg.windows_tap_interface)
            << std::endl;
#endif
  std::cout << std::endl;

  std::cout << utils::BOLD << "  Routes (" << cfg.routes.size()
            << "):" << utils::RESET << std::endl;
  for (const auto &r : cfg.routes)
    std::cout << "    • " << r << std::endl;

  if (!cfg.extra_args.empty()) {
    std::cout << std::endl
              << utils::BOLD << "  Extra Args:" << utils::RESET << std::endl;
    for (const auto &a : cfg.extra_args)
      std::cout << "    • " << a << std::endl;
  }

  std::cout << std::endl;
  std::cout << utils::DIM << "  Config : " << utils::get_config_path()
            << utils::RESET << std::endl;
  std::cout << utils::DIM << "  Key    : " << crypto::key_path() << "  ["
            << crypto::key_status() << "]" << utils::RESET << std::endl;
  std::cout << std::endl;
}

// ── get_plaintext_password ───────────────────────────────────────

std::string get_plaintext_password(const Config &cfg) {
  if (!cfg.remember_password) {
    utils::print_info("Password input is hidden and will not be displayed.");
    std::string pw = crypto::read_password_hidden("  VPN Password: ");
    if (pw.empty())
      utils::print_error("Password cannot be empty.");
    return pw;
  }
  if (cfg.password.empty()) {
    utils::print_error("Password not set. Run: exv config set password");
    return "";
  }
  std::string ks = crypto::key_status();
  if (ks != "valid") {
    utils::print_error("Encryption key is " + ks + "!");
    utils::print_info("Run 'exv config key reset' then re-set password.");
    logger::error("Password decrypt failed: key is " + ks);
    return "";
  }
  std::string key = crypto::load_key();
  std::string plaintext = crypto::decrypt(cfg.password, key);
  if (plaintext.empty()) {
    utils::print_error("Failed to decrypt password.");
    utils::print_info("Run 'exv config key reset' then re-set password.");
    logger::error("Password decryption returned empty");
  }
  return plaintext;
}
