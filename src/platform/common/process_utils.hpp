#pragma once

#include <string>

namespace ecnuvpn::platform {

int run_command(const std::string &cmd);
std::string run_command_output(const std::string &cmd);
std::string shell_quote(const std::string &value);
std::string get_executable_path();
bool check_root();

} // namespace ecnuvpn::platform

