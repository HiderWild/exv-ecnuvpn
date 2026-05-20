#include "platform/common/helper_platform.hpp"

namespace ecnuvpn {
namespace platform {

std::string helper_endpoint_path() {
  return "\\\\.\\pipe\\exv-helper";
}

std::string helper_state_path() {
  return "C:\\ProgramData\\exv-helper-session.json";
}

std::string stable_install_path() {
  return "C:\\Program Files\\ECNU-VPN\\exv.exe";
}

std::string stable_helper_install_path() {
  return "C:\\Program Files\\ECNU-VPN\\exv-helper.exe";
}

std::string helper_service_label() {
  return "exv-helper";
}

std::string helper_service_config_path() {
  return "";
}

} // namespace platform
} // namespace ecnuvpn
