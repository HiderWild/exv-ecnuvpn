#include <iostream>
#include <fstream>
#include <sstream>
#include <string>

#ifndef ECNUVPN_SOURCE_DIR
#error "ECNUVPN_SOURCE_DIR must be defined"
#endif

namespace {

std::string read_file(const std::string &path) {
  std::ifstream input(path);
  if (!input.good()) {
    std::cerr << "failed to read " << path << "\n";
    return {};
  }
  std::ostringstream buffer;
  buffer << input.rdbuf();
  return buffer.str();
}

bool contains(const std::string &text, const std::string &needle) {
  return text.find(needle) != std::string::npos;
}

} // namespace

int main() {
  const std::string cmake =
      read_file(std::string(ECNUVPN_SOURCE_DIR) + "/CMakeLists.txt");

  int failures = 0;
  const auto expect_contains = [&](const std::string &needle) {
    if (!contains(cmake, needle)) {
      std::cerr << "missing CMake UI shell policy: " << needle << "\n";
      ++failures;
    }
  };

  expect_contains("option(EXV_BUILD_UI_SHELL");
  expect_contains("WEBVIEW2_SDK_DIR is required");
  expect_contains("elseif(APPLE AND EXV_BUILD_UI_SHELL)");
  expect_contains("target_link_libraries(exv-ui PRIVATE \"-framework Cocoa\" \"-framework WebKit\")");
  expect_contains("if(UNIX AND NOT APPLE AND EXV_BUILD_UI_SHELL)");
  expect_contains("pkg_check_modules(WEBKITGTK");
  expect_contains("webkit2gtk-4.1");
  expect_contains("gtk+-3.0");

  return failures == 0 ? 0 : 1;
}
