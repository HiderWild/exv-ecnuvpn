#include "platform_network_ops.hpp"
#include <iostream>

namespace exv::platform {

class StubPlatformNetworkOps : public PlatformNetworkOps {
public:
    TunnelDeviceDescriptor prepare_tunnel_device(const std::string& adapter_name, int mtu) override {
        TunnelDeviceDescriptor desc;
        desc.path = "//./StubTun/" + adapter_name;
        desc.adapter_name = adapter_name;
        desc.mtu = mtu;
        desc.is_open = true;
        std::cout << "[StubPlatformNetworkOps] prepare_tunnel_device: " << adapter_name << std::endl;
        return desc;
    }

    TunnelDeviceDescriptor open_tunnel_device(const std::string& adapter_name) override {
        TunnelDeviceDescriptor desc;
        desc.path = "//./StubTun/" + adapter_name;
        desc.adapter_name = adapter_name;
        desc.is_open = true;
        return desc;
    }

    bool apply_tunnel_config(const TunnelDeviceDescriptor& device, const TunnelConfig& config) override {
        std::cout << "[StubPlatformNetworkOps] apply_tunnel_config: "
                  << config.interface_address << " with "
                  << config.routes.size() << " routes" << std::endl;
        return true;
    }

    CleanupResult cleanup(const std::string& adapter_name, CleanupPolicy policy) override {
        std::cout << "[StubPlatformNetworkOps] cleanup: " << adapter_name << std::endl;
        CleanupResult result;
        result.success = true;
        result.routes_removed = 2;
        result.dns_removed = true;
        result.adapter_removed = (policy == CleanupPolicy::Full);
        return result;
    }

    bool device_exists(const std::string& adapter_name) const override {
        return false;  // Stub always returns false
    }
};

std::unique_ptr<PlatformNetworkOps> PlatformNetworkOps::create() {
    return std::make_unique<StubPlatformNetworkOps>();
}

} // namespace exv::platform
