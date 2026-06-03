#pragma once
#include "../../src/platform/common/platform_network_ops.hpp"
#include <map>
#include <vector>

namespace exv::test {

class FakePlatformNetworkOps : public exv::platform::PlatformNetworkOps {
public:
    // PlatformNetworkOps interface
    platform::TunnelDeviceDescriptor prepare_tunnel_device(const std::string& adapter_name, int mtu = 1400) override;
    platform::TunnelDeviceDescriptor open_tunnel_device(const std::string& adapter_name) override;
    bool apply_tunnel_config(const platform::TunnelDeviceDescriptor& device, const platform::TunnelConfig& config) override;
    platform::CleanupResult cleanup(const std::string& adapter_name, platform::CleanupPolicy policy) override;
    bool device_exists(const std::string& adapter_name) const override;

    // Test control
    void set_prepare_should_fail(bool fail);
    void set_apply_should_fail(bool fail);
    void set_cleanup_should_fail(bool fail);
    void set_route_add_fail(bool fail);
    void set_dns_fail(bool fail);
    void set_adapter_create_fail(bool fail);
    void set_unsupported(bool unsupported);

    // Inspection
    int prepare_count() const;
    int apply_count() const;
    int cleanup_count() const;
    std::vector<platform::TunnelConfig> applied_configs() const;

private:
    bool prepare_fail_ = false;
    bool apply_fail_ = false;
    bool cleanup_fail_ = false;
    bool route_add_fail_ = false;
    bool dns_fail_ = false;
    bool adapter_create_fail_ = false;
    bool unsupported_ = false;
    int prepare_count_ = 0;
    int apply_count_ = 0;
    int cleanup_count_ = 0;
    std::map<std::string, platform::TunnelDeviceDescriptor> devices_;
    std::vector<platform::TunnelConfig> applied_configs_;
};

} // namespace exv::test
