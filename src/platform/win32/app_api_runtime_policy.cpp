#include "platform/common/app_api_runtime_policy.hpp"

namespace ecnuvpn {
namespace platform {

void prepare_direct_fallback_runtime() {}

std::string helper_unavailable_connect_message() {
  return "Helper daemon is not available. Install the helper service from Settings or run 'exv service install' as Administrator.";
}

std::string helper_unavailable_disconnect_message() {
  return "Helper daemon is not available. Use the elevated desktop action or install the helper service from Settings.";
}

} // namespace platform
} // namespace ecnuvpn