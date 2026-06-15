#pragma once

#include "helper/common/helper_messages.hpp"
#include "helper/runtime/cleanup_registry.hpp"

#include <memory>
#include <vector>

namespace exv::platform {
class PlatformNetworkOps;
}

namespace exv::helper {

class HelperNetworkOps {
public:
    virtual ~HelperNetworkOps() = default;

    virtual PrepareTunnelDeviceResponse prepare_tunnel_device(
        const PrepareTunnelDeviceRequest& request,
        std::vector<ManagedResource>* created_resources) = 0;

    virtual ApplyTunnelConfigResponse apply_tunnel_config(
        const ApplyTunnelConfigRequest& request,
        std::vector<ManagedResource>* created_resources) = 0;

    virtual CleanupResponse cleanup(
        const SessionId& session_id,
        const CleanupPolicy& policy,
        const std::vector<ManagedResource>& resources) = 0;
};

std::shared_ptr<HelperNetworkOps>
create_helper_network_ops(std::unique_ptr<platform::PlatformNetworkOps> platform_ops);
std::shared_ptr<HelperNetworkOps> create_helper_network_ops();

} // namespace exv::helper
