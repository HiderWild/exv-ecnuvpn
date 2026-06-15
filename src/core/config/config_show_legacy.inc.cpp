// ── Show ────────────────────────────────────────────────────────

void show(const Config &cfg) {
  cli::print_header("EXV Configuration");

  auto pw_status = [&]() -> std::string {
    if (!cfg.remember_password)
      return std::string(cli::DIM) + "(prompt at connect)" + cli::RESET;
    if (cfg.password.empty())
      return "(not set — run: exv config set password)";
    std::string ks = crypto::key_status();
    if (ks == "valid")
      return std::string(cli::GREEN) + "stored (encrypted)" + cli::RESET;
    return std::string(cli::RED) + "[KEY " + ks +
           " — run: exv config key reset]" + cli::RESET;
  };

  std::cout << cli::BOLD << "  Server          : " << cli::RESET
            << cfg.server << std::endl;
  std::cout << cli::BOLD << "  Username        : " << cli::RESET
            << (cfg.username.empty() ? "(not set)" : cfg.username) << std::endl;
  std::cout << cli::BOLD << "  Password        : " << cli::RESET
            << pw_status() << std::endl;
  std::cout << cli::BOLD << "  Remember Passwd : " << cli::RESET
            << (cfg.remember_password
                    ? std::string(cli::GREEN) + "yes" + cli::RESET
                    : std::string(cli::YELLOW) + "no (prompt on connect)" +
                          cli::RESET)
            << std::endl;
  std::cout << cli::BOLD << "  MTU             : " << cli::RESET << cfg.mtu
            << std::endl;
  std::cout << cli::BOLD << "  UserAgent       : " << cli::RESET
            << cfg.useragent << std::endl;
  std::cout << cli::BOLD << "  Disable DTLS    : " << cli::RESET
            << (cfg.disable_dtls
                    ? std::string(cli::YELLOW) + "yes (TLS-only transport)" +
                          cli::RESET
                    : std::string(cli::GREEN) + "no" + cli::RESET)
            << std::endl;
  std::cout << cli::BOLD << "  Log File        : " << cli::RESET
            << cfg.log_file << std::endl;
  std::cout << cli::BOLD << "  Auto Reconnect  : " << cli::RESET
            << (cfg.auto_reconnect ? "true" : "false") << std::endl;
  std::cout << cli::BOLD << "  Minimal Mode    : " << cli::RESET
            << (cfg.minimal_mode ? "true" : "false") << std::endl;
  std::cout << cli::BOLD << "  Minimal Install Service: " << cli::RESET
            << (cfg.minimal_install_service_before_connect ? "true" : "false")
            << std::endl;
  std::cout << cli::BOLD << "  VPN Engine      : " << cli::RESET
            << cfg.vpn_engine << std::endl;
  std::cout << cli::BOLD << "  OpenConnect Runtime: " << cli::RESET
            << cfg.openconnect_runtime << std::endl;
#ifdef _WIN32
  std::cout << cli::BOLD << "  Tunnel Driver   : " << cli::RESET
            << cfg.windows_tunnel_driver << std::endl;
  std::cout << cli::BOLD << "  TAP Interface   : " << cli::RESET
            << (cfg.windows_tap_interface.empty() ? "(auto)"
                                                  : cfg.windows_tap_interface)
            << std::endl;
#endif
  std::cout << std::endl;

  std::cout << cli::BOLD << "  Routes (" << cfg.routes.size()
            << "):" << cli::RESET << std::endl;
  for (const auto &r : cfg.routes)
    std::cout << "    • " << r << std::endl;

  if (!cfg.extra_args.empty()) {
    std::cout << std::endl
              << cli::BOLD << "  Extra Args:" << cli::RESET << std::endl;
    for (const auto &a : cfg.extra_args)
      std::cout << "    • " << a << std::endl;
  }

  std::cout << std::endl;
  std::cout << cli::DIM << "  Config : " << utils::get_config_path()
            << cli::RESET << std::endl;
  std::cout << cli::DIM << "  Key    : " << crypto::key_path() << "  ["
            << crypto::key_status() << "]" << cli::RESET << std::endl;
  std::cout << std::endl;
}

// ── get_plaintext_password ───────────────────────────────────────

std::string get_plaintext_password(const Config &cfg) {
  if (!cfg.remember_password) {
    cli::print_info("Password input is hidden and will not be displayed.");
    std::string pw = crypto::read_password_hidden("  VPN Password: ");
    if (pw.empty())
      cli::print_error("Password cannot be empty.");
    return pw;
  }
  if (cfg.password.empty()) {
    cli::print_error("Password not set. Run: exv config set password");
    return "";
  }
  std::string ks = crypto::key_status();
  if (ks != "valid") {
    cli::print_error("Encryption key is " + ks + "!");
    cli::print_info("Run 'exv config key reset' then re-set password.");
    logger::error("Password decrypt failed: key is " + ks);
    return "";
  }
  std::string key = crypto::load_key();
  std::string plaintext = crypto::decrypt(cfg.password, key);
  if (plaintext.empty()) {
    cli::print_error("Failed to decrypt password.");
    cli::print_info("Run 'exv config key reset' then re-set password.");
    logger::error("Password decryption returned empty");
  }
  return plaintext;
}
