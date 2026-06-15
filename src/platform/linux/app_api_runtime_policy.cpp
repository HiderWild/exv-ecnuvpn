#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/app_api_runtime_policy.hpp"

#include <sys/stat.h>

namespace ecnuvpn {
namespace platform {

void prepare_direct_fallback_runtime() {
  if (!platform::check_root())
    return;

  std::string home = platform::get_effective_home();
  if (home.empty())
    return;

  struct stat home_stat {};
  if (stat(home.c_str(), &home_stat) == 0) {
    platform::set_runtime_owner(home_stat.st_uid, home_stat.st_gid);
    platform::set_runtime_path_override(home, platform::get_config_dir());
  }
  platform::fix_runtime_config_dir_ownership();
}

std::string helper_unavailable_connect_message() {
  return "Helper daemon is not available. Install the helper service before starting the desktop client.";
}

std::string helper_unavailable_disconnect_message() {
  return "Helper daemon is not available. Install the helper service before disconnecting managed sessions.";
}

nlohmann::json preflight_connect_platform_checks(const ConfigView & /*cfg*/) {
  // No platform-specific driver checks on Linux.
  return nlohmann::json{};
}

nlohmann::json try_connect_direct_fallback(const ConfigView &cfg,
                                            const std::string &password) {
  (void)cfg;
  (void)password;
  return nlohmann::json{};
}

nlohmann::json try_disconnect_direct_fallback(bool allow_direct_fallback) {
  (void)allow_direct_fallback;
  return nlohmann::json{};
}

nlohmann::json status_fallback_without_helper(const ConfigView & /*cfg*/) {
  return nlohmann::json{};
}

} // namespace platform
} // namespace ecnuvpn
