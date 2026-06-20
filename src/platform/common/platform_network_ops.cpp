#include "platform_network_ops.hpp"

#if defined(EXV_PLATFORM_WINDOWS)
#include "platform/win32/platform_network_ops_win32.hpp"
#elif defined(EXV_PLATFORM_DARWIN)
#include "platform/darwin/platform_network_ops_darwin.hpp"
#elif defined(EXV_PLATFORM_LINUX)
#include "platform/linux/platform_network_ops_linux.hpp"
#endif

namespace exv::platform {

std::unique_ptr<PlatformNetworkOps> PlatformNetworkOps::create() {
#if defined(EXV_PLATFORM_WINDOWS)
    return create_win32_platform_network_ops();
#elif defined(EXV_PLATFORM_DARWIN)
    return create_darwin_platform_network_ops();
#elif defined(EXV_PLATFORM_LINUX)
    return create_linux_platform_network_ops();
#else
    return nullptr;
#endif
}

} // namespace exv::platform
