#pragma once

#include "virtual_network.hpp"

#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

std::vector<virtual_network::AdapterInfo>
detect_virtual_network_adapters(const std::string &exv_interface);

} // namespace platform
} // namespace ecnuvpn