#include "platform/common/helper_platform.hpp"

namespace exv {
namespace platform {

const HelperPlatformConfig &helper_platform_config() {
  static const HelperPlatformConfig config{
      "com.exv.helper",
      "com.exv.helper",
      "/Library/LaunchDaemons/com.exv.helper.plist",
      "/var/run/exv-helper.sock",
      "/var/run/exv-helper-session.json",
      "/usr/local/bin/exv",
      "/Library/Application Support/EXV/Helper/exv-helper",
      "launchd",
  };
  return config;
}

void wake_helper_daemon_for_shutdown() {}

} // namespace platform
} // namespace exv
