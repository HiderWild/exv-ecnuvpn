// ── Load / Save ──────────────────────────────────────────────────

Config load() {
  std::string dir = platform::get_config_dir();
  std::string path = platform::get_config_path();
  platform::ensure_dir(dir);

  if (!platform::fix_runtime_config_dir_ownership()) {
    cli::print_error("Configuration directory is owned by another user: " + dir);
    cli::print_info("Fix with: sudo chown -R $(whoami) " + dir);
    logger::error("Config dir ownership mismatch: " + dir);
    return Config{};
  }

  if (!platform::file_exists(path)) {
    Config cfg = run_wizard();
    if (!save(cfg)) {
      cli::print_error("Failed to save configuration to: " + path);
      cli::print_warning("Check directory permissions: " + dir);
      cli::print_info("If ~/.ecnuvpn is owned by root, fix with: sudo chown -R $(whoami) ~/.ecnuvpn");
      logger::error("Config save failed after wizard");
    }
    crypto::init_key_if_needed();
    logger::info("First-run setup wizard completed");
    return cfg;
  }

  crypto::init_key_if_needed();

  try {
    std::string content = platform::read_file(path);
    auto j = nlohmann::json::parse(content);
    return j.get<Config>();
  } catch (const std::exception &e) {
    cli::print_error("Failed to parse config: " + std::string(e.what()));
    cli::print_warning("Using default config.");
    logger::error("Config parse error: " + std::string(e.what()));
    return Config{};
  }
}

bool save(const Config &cfg) {
  std::string dir = platform::get_config_dir();
  std::string path = platform::get_config_path();
  platform::ensure_dir(dir);
  try {
    nlohmann::json j = cfg;
    if (platform::write_file(path, j.dump(4))) {
      logger::info("Config saved to: " + path);
      return true;
    }
  } catch (const std::exception &e) {
    cli::print_error("Failed to save config: " + std::string(e.what()));
    logger::error("Config save error: " + std::string(e.what()));
  }
  return false;
}
