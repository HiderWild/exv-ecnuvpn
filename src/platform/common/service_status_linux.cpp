#include "platform/common/service_status.hpp"

#include "helper.hpp"
#include "utils.hpp"

namespace ecnuvpn {
namespace platform {

ServiceStatusSnapshot current_service_status() {
  ServiceStatusSnapshot status;
  status.installed =
      utils::file_exists("/etc/systemd/system/exv-helper.service");
  status.available = helper::is_available();
  status.running = status.available;
  status.mode = "systemd";
  status.path = "/usr/local/bin/exv";
  status.endpoint = "/var/run/exv-helper.sock";
  status.label = "exv-helper.service";
  return status;
}

} // namespace platform
} // namespace ecnuvpn