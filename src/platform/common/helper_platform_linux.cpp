#include "platform/common/helper_platform.hpp"

namespace ecnuvpn {
namespace platform {

std::string helper_endpoint_path() {
  return "/var/run/exv-helper.sock";
}

std::string helper_state_path() {
  return "/var/run/exv-helper-session.json";
}

std::string stable_install_path() {
  return "/usr/local/bin/exv";
}

std::string stable_helper_install_path() {
  return "/usr/local/bin/exv";
}

std::string helper_service_label() {
  return "exv-helper";
}

std::string helper_service_config_path() {
  return "/etc/systemd/system/exv-helper.service";
}

} // namespace platform
} // namespace ecnuvpn
