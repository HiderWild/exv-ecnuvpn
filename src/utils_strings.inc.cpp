std::vector<std::string> split_lines(const std::string &text) {
  std::vector<std::string> lines;
  std::istringstream stream(text);
  std::string line;
  while (std::getline(stream, line)) {
    line = trim(line);
    if (!line.empty())
      lines.push_back(line);
  }
  return lines;
}

// ── String utilities ────────────────────────────────────────────

std::string trim(const std::string &s) {
  auto start = s.find_first_not_of(" \t\n\r");
  if (start == std::string::npos)
    return "";
  auto end = s.find_last_not_of(" \t\n\r");
  return s.substr(start, end - start + 1);
}

std::string get_log_path() {
  return platform::join_path(get_config_dir(), "ecnuvpn.log");
}

std::string get_tunnel_path() {
  return platform::tunnel_path(get_config_dir());
}
