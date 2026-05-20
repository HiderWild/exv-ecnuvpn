#pragma once

#include <string>

namespace ecnuvpn {
namespace platform {

void prepare_direct_fallback_runtime();
std::string helper_unavailable_connect_message();
std::string helper_unavailable_disconnect_message();

} // namespace platform
} // namespace ecnuvpn