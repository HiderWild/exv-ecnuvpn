#include "platform/common/helper_platform.hpp"

namespace ecnuvpn {
namespace platform {

const HelperPlatformConfig &helper_platform_config() {
  static const HelperPlatformConfig config{
      "exv-helper",
      "exv-helper.service",
      "/etc/systemd/system/exv-helper.service",
      "/var/run/exv-helper.sock",
      "/var/run/exv-helper-session.json",
      "/usr/local/bin/exv",
      "/usr/local/bin/exv",
      "systemd",
  };
  return config;
}

void wake_helper_daemon_for_shutdown() {}

} // namespace platform
} // namespace ecnuvpn