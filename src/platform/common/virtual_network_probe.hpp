#pragma once

#include "platform/common/virtual_network_model.hpp"

#include <string>
#include <vector>

namespace exv {
namespace platform {

std::vector<virtual_network::AdapterInfo>
detect_virtual_network_adapters(const std::string &exv_interface);

} // namespace platform
} // namespace exv
