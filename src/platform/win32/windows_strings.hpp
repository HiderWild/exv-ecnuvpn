#pragma once

#include <string>

namespace ecnuvpn::platform {

std::wstring wide_from_utf8(const std::string &value);
std::string utf8_from_wide(const std::wstring &value);
std::string windows_error_message(unsigned long error_code);

} // namespace ecnuvpn::platform

