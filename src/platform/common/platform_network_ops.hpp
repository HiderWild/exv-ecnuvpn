#pragma once
#include <string>
#include <memory>
#include "tunnel_config.hpp"
#include "tunnel_device_descriptor.hpp"

namespace exv::platform {

enum class CleanupPolicy {
    Full,           // Remove everything (adapter, routes, DNS, firewall)
    KeepAdapter,    // Keep adapter, remove routes/DNS/firewall
    RoutesOnly,     // Only remove routes
    DnsOnly         // Only remove DNS
};

struct CleanupResult {
    bool success = false;
    int routes_removed = 0;
    bool dns_removed = false;
    bool adapter_removed = false;
    bool firewall_removed = false;
    std::string error_message;
};

class PlatformNetworkOps {
public:
    virtual ~PlatformNetworkOps() = default;

    // Create/prepare tunnel device (e.g., create Wintun adapter)
    virtual TunnelDeviceDescriptor prepare_tunnel_device(const std::string& adapter_name, int mtu = 1400) = 0;

    // Open existing tunnel device for data plane I/O
    virtual TunnelDeviceDescriptor open_tunnel_device(const std::string& adapter_name) = 0;

    // Apply network configuration (routes, DNS, firewall)
    virtual bool apply_tunnel_config(const TunnelDeviceDescriptor& device, const TunnelConfig& config) = 0;

    // Cleanup resources
    virtual CleanupResult cleanup(const std::string& adapter_name, CleanupPolicy policy) = 0;

    // Check if device exists
    virtual bool device_exists(const std::string& adapter_name) const = 0;

    // Platform factory
    static std::unique_ptr<PlatformNetworkOps> create();
};

} // namespace exv::platform
