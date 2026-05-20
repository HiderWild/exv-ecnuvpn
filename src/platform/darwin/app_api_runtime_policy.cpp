#include "platform/common/app_api_runtime_policy.hpp"

#include "utils.hpp"

#include <sys/stat.h>

namespace ecnuvpn {
namespace platform {

void prepare_direct_fallback_runtime() {
  if (!utils::check_root())
    return;

  std::string home = utils::get_effective_home();
  if (home.empty())
    return;

  struct stat home_stat {};
  if (stat(home.c_str(), &home_stat) == 0) {
    utils::set_runtime_owner(home_stat.st_uid, home_stat.st_gid);
    utils::set_runtime_path_override(home, utils::get_config_dir());
  }
  utils::fix_config_dir_ownership();
}

std::string helper_unavailable_connect_message() {
  return "Helper daemon is not available. The desktop app can request one-time administrator authorization, or you can install the helper service for persistent connections.";
}

std::string helper_unavailable_disconnect_message() {
  return "Helper daemon is not available. The desktop app can request one-time administrator authorization to disconnect this session, or you can install the helper service.";
}

} // namespace platform
} // namespace ecnuvpn