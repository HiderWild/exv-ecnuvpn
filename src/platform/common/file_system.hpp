#pragma once

#include <string>

namespace ecnuvpn::platform {

bool file_exists(const std::string &path);
bool ensure_dir(const std::string &path);
std::string read_file(const std::string &path);
bool write_file(const std::string &path, const std::string &content);

} // namespace ecnuvpn::platform

