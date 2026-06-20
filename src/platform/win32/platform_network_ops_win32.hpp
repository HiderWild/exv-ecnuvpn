#pragma once

#include "platform/common/platform_network_ops.hpp"
#include "platform/win32/native_ip_config.hpp"
#include "platform/win32/native_wintun.hpp"

#include <memory>

namespace exv::platform {

std::unique_ptr<PlatformNetworkOps> create_win32_platform_network_ops();

std::unique_ptr<PlatformNetworkOps> create_win32_platform_network_ops(
    exv::platform::NativeWintunDependencies wintun_dependencies,
    exv::platform::NativeIpHelperApi ip_helper_api);

} // namespace exv::platform
