#pragma once

#include "platform/common/platform_network_ops.hpp"
#include "platform/darwin/native_route_config.hpp"
#include "platform/darwin/native_utun.hpp"

#include <memory>

namespace exv::platform {

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops();

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops(
    ecnuvpn::platform::NativeUtunApi utun_api,
    ecnuvpn::platform::NativeDarwinRouteApi route_api);

} // namespace exv::platform
