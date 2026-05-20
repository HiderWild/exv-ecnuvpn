#include "platform/common/helper_lifecycle.hpp"

namespace ecnuvpn {
namespace platform {

void cleanup_routes() {}

void kill_all_supervisors() {}

void fix_config_dir_ownership() {}

int copy_self_to_stable_path_and_reexec(const std::string &) {
  return 1;
}

} // namespace platform
} // namespace ecnuvpn
