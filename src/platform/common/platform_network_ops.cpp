#include "platform_network_ops.hpp"

namespace exv::platform {

std::unique_ptr<PlatformNetworkOps> PlatformNetworkOps::create() {
    return nullptr;
}

} // namespace exv::platform
