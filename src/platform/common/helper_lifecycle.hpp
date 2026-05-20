#pragma once

#include <string>

namespace ecnuvpn {
namespace platform {

void cleanup_routes();
void kill_all_supervisors();
void fix_config_dir_ownership();
int copy_self_to_stable_path_and_reexec(const std::string &current_path);

} // namespace platform
} // namespace ecnuvpn
