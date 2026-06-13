void print_success(const std::string &msg) {
  std::cout << GREEN << "✅ " << msg << RESET << std::endl;
}

void print_error(const std::string &msg) {
  std::cerr << RED << "❌ " << msg << RESET << std::endl;
}

void print_info(const std::string &msg) {
  std::cout << CYAN << "ℹ️  " << msg << RESET << std::endl;
}

void print_warning(const std::string &msg) {
  std::cout << YELLOW << "⚠️  " << msg << RESET << std::endl;
}

void print_header(const std::string &msg) {
  std::cout << std::endl;
  std::cout << BOLD << MAGENTA << "╔══════════════════════════════════════════╗"
            << RESET << std::endl;
  std::cout << BOLD << MAGENTA << "║  " << msg;
  // Pad to fill the box width
  int pad = 40 - static_cast<int>(msg.size());
  for (int i = 0; i < pad; ++i)
    std::cout << ' ';
  std::cout << RESET << BOLD << MAGENTA << "║" << RESET << std::endl;
  std::cout << BOLD << MAGENTA << "╚══════════════════════════════════════════╝"
            << RESET << std::endl;
  std::cout << std::endl;
}

