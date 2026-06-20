#include "platform/win32/windows_strings.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

namespace exv::platform {

std::wstring wide_from_utf8(const std::string &value) {
  if (value.empty()) {
    return {};
  }
  int length = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.c_str(),
                                   static_cast<int>(value.size()), nullptr, 0);
  if (length <= 0) {
    length = MultiByteToWideChar(CP_ACP, 0, value.c_str(),
                                 static_cast<int>(value.size()), nullptr, 0);
  }
  if (length <= 0) {
    return {};
  }

  std::wstring result(static_cast<std::size_t>(length), L'\0');
  UINT codepage = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                      value.c_str(),
                                      static_cast<int>(value.size()), nullptr,
                                      0) > 0
                      ? CP_UTF8
                      : CP_ACP;
  MultiByteToWideChar(codepage, codepage == CP_UTF8 ? MB_ERR_INVALID_CHARS : 0,
                      value.c_str(), static_cast<int>(value.size()),
                      result.data(), length);
  return result;
}

std::string utf8_from_wide(const std::wstring &value) {
  if (value.empty()) {
    return {};
  }
  int length = WideCharToMultiByte(CP_UTF8, 0, value.c_str(),
                                   static_cast<int>(value.size()), nullptr, 0,
                                   nullptr, nullptr);
  if (length <= 0) {
    return {};
  }

  std::string result(static_cast<std::size_t>(length), '\0');
  WideCharToMultiByte(CP_UTF8, 0, value.c_str(),
                      static_cast<int>(value.size()), result.data(), length,
                      nullptr, nullptr);
  return result;
}

std::string windows_error_message(unsigned long error_code) {
  wchar_t *buffer = nullptr;
  DWORD flags = FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM |
                FORMAT_MESSAGE_IGNORE_INSERTS;
  DWORD length = FormatMessageW(flags, nullptr, static_cast<DWORD>(error_code),
                                MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT),
                                reinterpret_cast<LPWSTR>(&buffer), 0, nullptr);
  if (length == 0 || !buffer) {
    return std::to_string(error_code);
  }

  std::wstring message(buffer, length);
  LocalFree(buffer);
  while (!message.empty() &&
         (message.back() == L'\r' || message.back() == L'\n' ||
          message.back() == L'.' || message.back() == L' ')) {
    message.pop_back();
  }
  return utf8_from_wide(message) + " (" + std::to_string(error_code) + ")";
}

} // namespace exv::platform

