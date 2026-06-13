// ── Reset ───────────────────────────────────────────────────────

Config reset() {
  Config cfg;
  save(cfg);
  tunnel::write_script(cfg);
  utils::print_success("Config reset to defaults. Key file preserved.");
  utils::print_info("Run 'exv config set password' to set a new password.");
  logger::info("Config reset to defaults");
  return cfg;
}

// ── Route management ────────────────────────────────────────────

bool add_route(Config &cfg, const std::string &route) {
  if (std::find(cfg.routes.begin(), cfg.routes.end(), route) !=
      cfg.routes.end()) {
    utils::print_warning("Route already exists: " + route);
    return false;
  }
  cfg.routes.push_back(route);
  save(cfg);
  utils::print_success("Route added: " + route);
  logger::info("Route added: " + route);
  return true;
}

bool remove_route(Config &cfg, const std::string &route) {
  auto it = std::find(cfg.routes.begin(), cfg.routes.end(), route);
  if (it == cfg.routes.end()) {
    utils::print_error("Route not found: " + route);
    return false;
  }
  cfg.routes.erase(it);
  save(cfg);
  utils::print_success("Route removed: " + route);
  logger::info("Route removed: " + route);
  return true;
}

void list_routes(const Config &cfg) {
  utils::print_header("VPN Routes");
  if (cfg.routes.empty()) {
    utils::print_warning("No routes configured.");
    return;
  }
  std::cout << "  Total: " << cfg.routes.size() << " routes" << std::endl
            << std::endl;
  for (size_t i = 0; i < cfg.routes.size(); ++i)
    std::cout << "  " << utils::GREEN << (i + 1) << "." << utils::RESET << " "
              << cfg.routes[i] << std::endl;
  std::cout << std::endl;
}

// ── Key management ──────────────────────────────────────────────

void key_show() {
  utils::print_header("Encryption Key Status");
  std::string ks = crypto::key_status();
  std::cout << "  Key file : " << crypto::key_path() << std::endl;
  std::cout << "  Status   : ";
  if (ks == "valid")
    std::cout << utils::GREEN << utils::BOLD << "valid" << utils::RESET
              << std::endl;
  else if (ks == "missing") {
    std::cout << utils::YELLOW << utils::BOLD << "missing" << utils::RESET
              << std::endl;
    utils::print_info("Run: exv config key reset");
  } else {
    std::cout << utils::RED << utils::BOLD << "corrupt" << utils::RESET
              << std::endl;
    utils::print_warning("Run: exv config key reset");
  }
  std::cout << std::endl;
}

bool key_reset() { return crypto::reset_key(); }
