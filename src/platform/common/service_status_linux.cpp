#include "platform/common/service_status.hpp"

#include "helper/helper.hpp"
#include "platform/common/helper_platform.hpp"
#include "utils.hpp"

namespace ecnuvpn {
namespace platform {

ServiceStatusSnapshot current_service_status() {
  const auto &config = helper_platform_config();
  ServiceStatusSnapshot status;
  status.installed = utils::file_exists(config.service_definition_path);
  status.available = helper::is_available();
  status.running = status.available;
  status.mode = config.service_mode;
  status.path = config.default_service_binary_path;
  status.endpoint = config.endpoint;
  status.label = config.service_label;
  status.capabilities = nlohmann::json{{"service_mode", true},
                                       {"oneshot_mode", false},
                                       {"temporary_connect", false},
                                       {"direct_fallback", false},
                                       {"helper_binary", true}};
  return status;
}

} // namespace platform
} // namespace ecnuvpn
