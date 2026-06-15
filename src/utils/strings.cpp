#include "utils/strings.hpp"

#include <cstddef>
#include <utility>

namespace exv::utils {
namespace {

constexpr bool is_ascii_space(char c) noexcept {
  return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

} // namespace

std::string trim(std::string_view value) {
  std::size_t begin = 0;
  while (begin < value.size() && is_ascii_space(value[begin])) {
    ++begin;
  }
  if (begin == value.size()) {
    return {};
  }

  std::size_t end = value.size();
  while (end > begin && is_ascii_space(value[end - 1])) {
    --end;
  }
  return std::string{value.substr(begin, end - begin)};
}

std::vector<std::string> split_lines(std::string_view text) {
  std::vector<std::string> lines;
  std::size_t line_begin = 0;

  while (line_begin <= text.size()) {
    const auto line_end = text.find('\n', line_begin);
    const auto count = line_end == std::string_view::npos
                           ? text.size() - line_begin
                           : line_end - line_begin;
    std::string line = trim(text.substr(line_begin, count));
    if (!line.empty()) {
      lines.push_back(std::move(line));
    }

    if (line_end == std::string_view::npos) {
      break;
    }
    line_begin = line_end + 1;
  }

  return lines;
}

} // namespace exv::utils
