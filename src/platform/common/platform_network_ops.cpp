#include "platform_network_ops.hpp"

namespace exv::platform {

class UnavailablePlatformNetworkOps : public PlatformNetworkOps {
public:
    TunnelDeviceDescriptor prepare_tunnel_device(const std::string& adapter_name, int mtu) override {
        TunnelDeviceDescriptor desc;
        desc.adapter_name = adapter_name;
        desc.mtu = mtu;
        desc.is_open = false;
        return desc;
    }

    TunnelDeviceDescriptor open_tunnel_device(const std::string& adapter_name) override {
        TunnelDeviceDescriptor desc;
        desc.adapter_name = adapter_name;
        desc.is_open = false;
        return desc;
    }

    bool apply_tunnel_config(const TunnelDeviceDescriptor& device, const TunnelConfig& config) override {
        (void)device;
        (void)config;
        return false;
    }

    CleanupResult cleanup(const std::string& adapter_name, CleanupPolicy policy) override {
        (void)adapter_name;
        (void)policy;
        CleanupResult result;
        result.success = false;
        result.error_message = "PlatformNetworkOps unavailable";
        return result;
    }

    bool device_exists(const std::string& adapter_name) const override {
        (void)adapter_name;
        return false;
    }
};

std::unique_ptr<PlatformNetworkOps> PlatformNetworkOps::create() {
    return std::make_unique<UnavailablePlatformNetworkOps>();
}

} // namespace exv::platform
