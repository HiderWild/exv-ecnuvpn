#pragma once

#include <string>
#include <string_view>

namespace exv::setup {

// Quote one argv value using the CommandLineToArgvW / Windows CRT backslash rules.
inline std::wstring QuoteCommandLineArgument(std::wstring_view value) {
  std::wstring quoted;
  quoted.push_back(L'"');
  std::size_t backslashes = 0;
  for (const wchar_t ch : value) {
    if (ch == L'\\') {
      ++backslashes;
      continue;
    }
    if (ch == L'"') {
      quoted.append(backslashes * 2 + 1, L'\\');
      quoted.push_back(L'"');
      backslashes = 0;
      continue;
    }
    quoted.append(backslashes, L'\\');
    backslashes = 0;
    quoted.push_back(ch);
  }
  // Backslashes immediately before the closing quote must be doubled.
  quoted.append(backslashes * 2, L'\\');
  quoted.push_back(L'"');
  return quoted;
}

}  // namespace exv::setup
