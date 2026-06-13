static std::string runtime_home_override;
static std::string runtime_config_dir_override;
static uid_t runtime_owner_uid = static_cast<uid_t>(-1);
static gid_t runtime_owner_gid = static_cast<gid_t>(-1);

static std::string expand_home_with_base(const std::string &path,
                                         const std::string &home) {
  if (!path.empty() && path[0] == '~' && !home.empty()) {
    return home + path.substr(1);
  }
  return path;
}

static std::string get_config_dir_for_home(const std::string &home) {
  std::string default_dir = platform::default_config_dir_for_home(home);
  if (default_dir.empty())
    return "";

  std::string redirect = platform::redirect_path_for_home(home);
  if (!redirect.empty()) {
    std::ifstream rf(redirect);
    if (rf.is_open()) {
      std::string dir;
      std::getline(rf, dir);
      dir = trim(dir);
      if (!dir.empty())
        return expand_home_with_base(dir, home);
    }
  }

  return default_dir;
}

static std::vector<std::string> candidate_runtime_dirs() {
  std::vector<std::string> dirs;

  const char *env_runtime_dir = std::getenv("ECNUVPN_RUNTIME_DIR");
  if (env_runtime_dir && *env_runtime_dir) {
    dirs.push_back(env_runtime_dir);
  }

  std::string exec_path = get_executable_path();
  if (!exec_path.empty()) {
    std::filesystem::path exec_dir = std::filesystem::path(exec_path).parent_path();
    dirs.push_back(exec_dir.string());
    dirs.push_back(platform::join_path(exec_dir.string(), "runtime"));
    dirs.push_back(platform::join_path(exec_dir.string(), "openconnect"));
    dirs.push_back(platform::join_path(
        platform::join_path(exec_dir.string(), "runtime"), "openconnect"));
  }

  return dirs;
}

static std::string first_existing_file(const std::vector<std::string> &paths) {
  for (const auto &path : paths) {
    if (!path.empty() && file_exists(path))
      return path;
  }
  return "";
}

// ── Colored output ──────────────────────────────────────────────
