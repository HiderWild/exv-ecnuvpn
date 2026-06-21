#pragma once

#include <string>

namespace exv::platform {

int run_command(const std::string &cmd);
std::string run_command_output(const std::string &cmd);
std::string shell_quote(const std::string &value);
std::string get_executable_path();
std::string powershell_command();
std::string powershell_encoded_command(const std::string &script,
                                       const std::string &arguments);
bool check_root();

} // namespace exv::platform

