std::string js_quote(const std::string &value) {
  std::string quoted = "\"";
  for (char c : value) {
    switch (c) {
    case '\\':
      quoted += "\\\\";
      break;
    case '"':
      quoted += "\\\"";
      break;
    case '\r':
      quoted += "\\r";
      break;
    case '\n':
      quoted += "\\n";
      break;
    default:
      quoted += c;
      break;
    }
  }
  quoted += '"';
  return quoted;
}

std::string env_value(const char *name, const std::string &fallback = "") {
  const char *value = std::getenv(name);
  if (!value || !*value)
    return fallback;
  return value;
}

bool is_numeric(const std::string &value) {
  if (value.empty())
    return false;
  for (char c : value) {
    if (c < '0' || c > '9')
      return false;
  }
  return true;
}

std::string cmd_quote_arg(const std::string &value) {
  std::string quoted = "\"";
  for (char c : value) {
    if (c == '"')
      quoted += "\\\"";
    else
      quoted += c;
  }
  quoted += "\"";
  return quoted;
}

void debug_log(const std::string &ready_path, const std::string &message) {
  std::ofstream out(ready_path + ".debug.log", std::ios::app);
  if (out.is_open())
    out << message << "\n";
}

int run_exit(const std::string &ready_path, const std::string &cmd) {
  debug_log(ready_path, "run: " + cmd);
  int rc = utils::run_command(cmd);
  debug_log(ready_path, "exit " + std::to_string(rc) + ": " + cmd);
  return rc;
}

bool run_with_retry(const std::string &ready_path, const std::string &cmd,
                    int max_retries, unsigned int delay_ms,
                    bool ignore_failure) {
  for (int attempt = 1; attempt <= max_retries; ++attempt) {
    int rc = run_exit(ready_path, cmd);
    if (rc == 0)
      return true;
    if (attempt < max_retries)
      Sleep(delay_ms);
  }
  return ignore_failure;
}

std::string effective_mtu(const std::string &reported_mtu, int configured_mtu) {
  int reported = 0;
  try {
    reported = reported_mtu.empty() ? 0 : std::stoi(reported_mtu);
  } catch (...) {
    reported = 0;
  }

  if (reported >= 1200)
    return std::to_string(reported);
  if (configured_mtu >= 1200)
    return std::to_string(configured_mtu);
  return "";
}

std::string get_default_gateway4() {
  std::string output = utils::run_command_output("route.exe print 0.0.0.0");
  std::regex route_regex(R"(0\.0\.0\.0\s+(?:0|128)\.0\.0\.0\s+([0-9.]+))");
  std::smatch match;
  if (std::regex_search(output, match, route_regex) && match.size() > 1)
    return match[1].str();
  return "";
}

void delete_ready_file(const std::string &ready_path) {
  std::remove(ready_path.c_str());
}

bool write_ready_file(const std::string &ready_path, const std::string &tundev,
                      const std::string &internal_ip) {
  std::ofstream out(ready_path, std::ios::trunc);
  if (!out.is_open())
    return false;
  out << tundev << "\n" << internal_ip << "\n";
  return true;
}

std::pair<std::string, std::string>
cidr_to_network_and_mask(const std::string &cidr);
