#include "platform/common/service_status.hpp"

#include "helper.hpp"
#include "utils.hpp"

namespace ecnuvpn {
namespace platform {

ServiceStatusSnapshot current_service_status() {
  ServiceStatusSnapshot status;
  status.installed =
      utils::file_exists("/Library/LaunchDaemons/com.ecnu.exv.helper.plist");
  status.available = helper::is_available();
  status.running = status.available;
  status.mode = "launchd";
  status.path = "/usr/local/bin/exv";
  status.endpoint = "/var/run/exv-helper.sock";
  status.label = "com.ecnu.exv.helper";
  return status;
}

} // namespace platform
} // namespace ecnuvpn