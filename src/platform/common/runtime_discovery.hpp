#pragma once

#include <string>

namespace ecnuvpn::platform {

std::string get_bundled_runtime_dir();
std::string get_bundled_openconnect_path();
std::string get_bundled_wintun_path();
std::string get_bundled_tap_installer_path();
std::string get_openconnect_path(const std::string &runtime_mode = "auto");
bool check_openconnect(const std::string &runtime_mode = "auto");

} // namespace ecnuvpn::platform

