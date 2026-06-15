// ── Setup wizard ─────────────────────────────────────────────────

static Config run_wizard() {
  wiz_banner();

  std::cout << "  No configuration found!" << std::endl;
  std::cout << "  Let's get you set up." << std::endl << std::endl;

  std::cout << "  Choose a setup mode:" << std::endl;
  std::cout << "    " << cli::GREEN << "[1]" << cli::RESET
            << " Easy Mode     — quick setup with defaults  " << cli::DIM
            << "(recommended)" << cli::RESET << std::endl;
  std::cout << "    " << cli::YELLOW << "[2]" << cli::RESET
            << " Advanced Mode — customize all settings" << std::endl;
  std::cout << std::endl << "  Choice [1]: ";
  std::string mode_input;
  std::getline(std::cin, mode_input);
  mode_input = exv::utils::trim(mode_input);
  bool advanced = (!mode_input.empty() && mode_input[0] == '2');
  std::cout << std::endl;

  Config cfg;

  if (!advanced) {
    wiz_step(1, 2, "Account");
    cfg.username = wiz_prompt("Username (student ID)", cfg.username);

    wiz_step(2, 2, "Password");
    cli::print_info("Password input is hidden and will not be displayed.");
    cfg.password = crypto::read_password_hidden("    Password: ");
    wiz_progress(2, 2);

    crypto::init_key_if_needed();
    if (!cfg.password.empty()) {
      std::string key = crypto::load_key();
      cfg.password = crypto::encrypt(cfg.password, key);
      cfg.remember_password = true;
    }

  } else {
    constexpr int TOTAL = 6;

    wiz_step(1, TOTAL, "Working Directory");
    std::cout << "    Where should exv store its files?" << std::endl;
    std::string default_dir = platform::get_config_dir();
    std::string new_dir = wiz_prompt("Directory", default_dir);
    if (new_dir != default_dir) {
      if (!platform::set_config_dir(new_dir))
        cli::print_warning("Could not create " + new_dir +
                             ". Using default.");
      else
        cli::print_success("Work directory: " + platform::expand_home(new_dir));
    }
    cfg.log_file = new_dir + "/ecnuvpn.log";

    wiz_step(2, TOTAL, "VPN Server");
    cfg.server = wiz_prompt("Server URL", cfg.server);

    wiz_step(3, TOTAL, "Account");
    cfg.username = wiz_prompt("Username (student ID)", cfg.username);

    wiz_step(4, TOTAL, "Remember Password");
    std::cout << "    Should exv save your password (encrypted)?"
              << std::endl;
    std::cout
        << "    Choosing no means you will be prompted every time you connect."
        << std::endl;
    cfg.remember_password = wiz_confirm("Remember password?", true);

    crypto::init_key_if_needed();

    wiz_step(5, TOTAL, "Split Tunnel Routes");
    std::cout << "    Toggle numbers to select/deselect routes." << std::endl;
    std::cout
        << "    Enter an IP or CIDR (e.g. 10.0.0.0/8) to add a custom route."
        << std::endl;
    std::cout << "    Press " << cli::BOLD << "Enter" << cli::RESET
              << " on an empty line to confirm." << std::endl;
    cfg.routes = wiz_route_selector(cfg.routes);
    std::cout << std::endl;
    cli::print_success("Routes configured: " +
                         std::to_string(cfg.routes.size()) + " selected.");

    wiz_step(6, TOTAL, "Password");
    if (cfg.remember_password) {
      cli::print_info("Password input is hidden and will not be displayed.");
      std::string pw = crypto::read_password_hidden("    Password: ");
      if (!pw.empty()) {
        std::string key = crypto::load_key();
        cfg.password = crypto::encrypt(pw, key);
      }
    } else {
      cli::print_info("Password will be prompted each time you connect.");
      cfg.password = "";
    }
    wiz_progress(TOTAL, TOTAL);
  }

  std::cout << std::endl;
  cli::print_success("Setup complete!");
  std::cout << std::endl;
  return cfg;
}
