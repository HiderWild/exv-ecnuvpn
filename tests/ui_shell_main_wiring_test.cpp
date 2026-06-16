#include <fstream>
#include <iostream>
#include <iterator>
#include <string>

#ifndef ECNUVPN_SOURCE_DIR
#error "ECNUVPN_SOURCE_DIR must be defined"
#endif

namespace {

std::string read_file(const std::string &path) {
  std::ifstream input(path);
  return {std::istreambuf_iterator<char>(input),
          std::istreambuf_iterator<char>()};
}

bool contains(const std::string &text, const std::string &needle) {
  return text.find(needle) != std::string::npos;
}

} // namespace

int main() {
  const std::string root = ECNUVPN_SOURCE_DIR;
  const std::string main_cpp =
      read_file(root + "/src/app/ui_shell/ui_shell_main.cpp");
  int failures = 0;

  if (!contains(main_cpp, "run_ui_shell_window(")) {
    std::cerr << "exv-ui main must run the neutral runtime\n";
    ++failures;
  }
  if (!contains(main_cpp, "create_core_process_transport(")) {
    std::cerr << "exv-ui main must create a production core RPC transport\n";
    ++failures;
  }
  if (contains(main_cpp,
               "return ecnuvpn::platform::win32::ui_shell::"
               "run_webview2_host(config);")) {
    std::cerr << "exv-ui main must not bypass run_ui_shell_window on Windows\n";
    ++failures;
  }

  return failures == 0 ? 0 : 1;
}
