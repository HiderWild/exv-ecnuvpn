#include "utils/strings.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

} // namespace

int main() {
  bool ok = true;

  ok = expect(exv::utils::trim(" \t hello \r\n") == "hello",
              "trim removes ASCII edge whitespace") &&
       ok;
  ok = expect(exv::utils::trim(" \t\r\n") == "",
              "trim returns empty for all whitespace") &&
       ok;

  const auto lines = exv::utils::split_lines(" alpha\r\n\n beta \n\t\n");
  ok = expect(lines == std::vector<std::string>{"alpha", "beta"},
              "split_lines trims and drops blank lines") &&
       ok;

  ok = expect(exv::utils::split_lines("").empty(),
              "split_lines returns empty for empty input") &&
       ok;

  return ok ? 0 : 1;
}

