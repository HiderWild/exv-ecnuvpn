std::string get_bundled_runtime_dir() {
  std::vector<std::string> dirs = candidate_runtime_dirs();
  for (const auto &dir : dirs) {
    if (!dir.empty() && file_exists(dir))
      return dir;
  }
  return "";
}

std::string get_bundled_openconnect_path() {
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (dir.empty())
      continue;
#ifdef _WIN32
  candidates.push_back(platform::join_path(dir, "openconnect.exe"));
#else
  candidates.push_back(platform::join_path(dir, "openconnect"));
#endif
  }
  return first_existing_file(candidates);
}

std::string get_bundled_wintun_path() {
#ifdef _WIN32
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (dir.empty())
      continue;
    candidates.push_back(platform::join_path(dir, "wintun.dll"));
  }
  return first_existing_file(candidates);
#else
  return "";
#endif
}

std::string get_bundled_tap_installer_path() {
#ifdef _WIN32
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (dir.empty())
      continue;
    candidates.push_back(platform::join_path(dir, "tap-windows-installer.exe"));
    candidates.push_back(platform::join_path(dir, "tap-windows-amd64.exe"));
    candidates.push_back(platform::join_path(dir, "tap-windows-x86.exe"));
    candidates.push_back(platform::join_path(dir, "tap-windows/OemVista.inf"));
    candidates.push_back(platform::join_path(dir, "tap/OemVista.inf"));
  }
  return first_existing_file(candidates);
#else
  return "";
#endif
}

std::string get_openconnect_path(const std::string &runtime_mode) {
  const char *env_openconnect = std::getenv("ECNUVPN_OPENCONNECT");
  if (env_openconnect && *env_openconnect && file_exists(env_openconnect))
    return env_openconnect;

  if (runtime_mode != "system") {
    std::string bundled = get_bundled_openconnect_path();
    if (!bundled.empty()) {
#ifdef __APPLE__
      std::string verify_cmd = "codesign --verify --strict " +
                               shell_quote(bundled) + " >/dev/null 2>&1";
      if (std::system(verify_cmd.c_str()) != 0)
        bundled.clear();
#endif
    }
    if (!bundled.empty())
      return bundled;
  }

#ifdef __APPLE__
  const char *candidates[] = {"/opt/homebrew/bin/openconnect",
                              "/usr/local/bin/openconnect",
                              "/usr/bin/openconnect",
                              "/bin/openconnect"};
#elif defined(_WIN32)
  const char *candidates[] = {
      "C:\\Program Files\\OpenConnect\\openconnect.exe",
      "C:\\Program Files (x86)\\OpenConnect\\openconnect.exe",
      "openconnect.exe"};
#else
  const char *candidates[] = {"/usr/sbin/openconnect",
                              "/usr/bin/openconnect",
                              "/sbin/openconnect",
                              "/usr/local/bin/openconnect"};
#endif
  for (const char *candidate : candidates) {
#ifdef _WIN32
    if (candidate && _access(candidate, 0) == 0)
#else
    if (candidate && access(candidate, X_OK) == 0)
#endif
      return candidate;
  }

#ifdef _WIN32
  std::string resolved = trim(run_command_output("where openconnect.exe 2>nul"));
  std::string::size_type newline = resolved.find_first_of("\r\n");
  if (newline != std::string::npos)
    resolved.resize(newline);
  if (!resolved.empty() && _access(resolved.c_str(), 0) == 0)
    return resolved;
#else
  std::string resolved =
      trim(run_command_output("command -v openconnect 2>/dev/null"));
  if (!resolved.empty() && access(resolved.c_str(), X_OK) == 0)
    return resolved;
#endif
  return "";
}

bool check_openconnect(const std::string &runtime_mode) {
  return !get_openconnect_path(runtime_mode).empty();
}

bool check_root() {
#ifndef _WIN32
  return geteuid() == 0;
#else
  // On Windows, check if the process is running as Administrator
  SID_IDENTIFIER_AUTHORITY nt_auth = SECURITY_NT_AUTHORITY;
  PSID admin_group = nullptr;
  BOOL is_admin = FALSE;
  if (AllocateAndInitializeSid(&nt_auth, 2,
        SECURITY_BUILTIN_DOMAIN_RID, DOMAIN_ALIAS_RID_ADMINS,
        0, 0, 0, 0, 0, 0, &admin_group)) {
    CheckTokenMembership(nullptr, admin_group, &is_admin);
    FreeSid(admin_group);
  }
  return is_admin ? true : false;
#endif
}
