#pragma once

#include <string>
#include <string_view>
#include <vector>

namespace exv::utils {

std::string trim(std::string_view value);
std::vector<std::string> split_lines(std::string_view text);

} // namespace exv::utils

