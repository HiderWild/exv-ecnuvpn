#include "platform/common/helper_platform.hpp"

namespace ecnuvpn {
namespace platform {

const HelperPlatformConfig &helper_platform_config() {
  static const HelperPlatformConfig config{
      "com.ecnu.exv.helper",
      "com.ecnu.exv.helper",
      "/Library/LaunchDaemons/com.ecnu.exv.helper.plist",
      "/var/run/exv-helper.sock",
      "/var/run/exv-helper-session.json",
      "/usr/local/bin/exv",
      "/usr/local/bin/exv-helper",
      "launchd",
  };
  return config;
}

void wake_helper_daemon_for_shutdown() {}

} // namespace platform
} // namespace ecnuvpn
