#include <iostream>
#include <string>
#include <vector>

import exv.utils.strings;

int main() {
  const bool ok =
      exv::utils::trim(" value ") == "value" &&
      exv::utils::split_lines("a\n\n b\r\n") ==
          std::vector<std::string>{"a", "b"};
  if (!ok) {
    std::cerr << "utils strings module smoke test failed\n";
  }
  return ok ? 0 : 1;
}
