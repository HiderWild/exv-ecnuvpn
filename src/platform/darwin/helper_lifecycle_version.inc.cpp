bool prompt_confirm(const std::string &question, bool default_yes) {
  if (!isatty(STDIN_FILENO))
    return true;

  std::cout << "  " << question << (default_yes ? " [Y/n]: " : " [y/N]: ");
  std::string input;
  std::getline(std::cin, input);
  input = utils::trim(input);
  if (input.empty())
    return default_yes;
  return input[0] == 'y' || input[0] == 'Y';
}

bool parse_semantic_version_token(const std::string &token,
                                  SemanticVersion *version) {
  if (!version || token.empty())
    return false;

  std::istringstream iss(token);
  std::string part;
  std::vector<int> parts;
  while (std::getline(iss, part, '.')) {
    if (part.empty())
      return false;
    for (char ch : part) {
      if (!std::isdigit(static_cast<unsigned char>(ch)))
        return false;
    }
    try {
      parts.push_back(std::stoi(part));
    } catch (...) {
      return false;
    }
  }

  if (parts.size() != 3)
    return false;

  version->major = parts[0];
  version->minor = parts[1];
  version->patch = parts[2];
  return true;
}

bool parse_semantic_version(const std::string &text, SemanticVersion *version) {
  if (!version)
    return false;

  std::string candidate;
  auto flush_candidate = [&]() -> bool {
    if (candidate.empty())
      return false;
    SemanticVersion parsed;
    bool ok = parse_semantic_version_token(candidate, &parsed);
    candidate.clear();
    if (ok) {
      *version = parsed;
      return true;
    }
    return false;
  };

  for (char ch : text) {
    unsigned char uch = static_cast<unsigned char>(ch);
    if (std::isdigit(uch) || ch == '.') {
      candidate.push_back(ch);
    } else if (flush_candidate()) {
      return true;
    }
  }

  return flush_candidate();
}

std::string format_semantic_version(const SemanticVersion &version) {
  return std::to_string(version.major) + "." +
         std::to_string(version.minor) + "." +
         std::to_string(version.patch);
}

int compare_semantic_versions(const SemanticVersion &lhs,
                              const SemanticVersion &rhs) {
  if (lhs.major != rhs.major)
    return lhs.major < rhs.major ? -1 : 1;
  if (lhs.minor != rhs.minor)
    return lhs.minor < rhs.minor ? -1 : 1;
  if (lhs.patch != rhs.patch)
    return lhs.patch < rhs.patch ? -1 : 1;
  return 0;
}

bool read_binary_version(const std::string &path, SemanticVersion *version) {
  if (!version || !utils::file_exists(path))
    return false;

  std::string output = utils::trim(
      utils::run_command_output(utils::shell_quote(path) + " version 2>/dev/null"));
  if (output.empty())
    return false;
  return parse_semantic_version(output, version);
}

