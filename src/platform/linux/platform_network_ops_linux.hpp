#pragma once

#include "platform/common/platform_network_ops.hpp"

#include <memory>

namespace exv::platform {

std::unique_ptr<PlatformNetworkOps> create_linux_platform_network_ops();

} // namespace exv::platform
