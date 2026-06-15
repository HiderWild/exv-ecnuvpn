#pragma once
#include <string>
#include <vector>
#include <set>

namespace exv::helper {

enum class Capability : uint32_t {
    TunnelDeviceCreate = 1,
    TunnelDeviceOpen = 2,
    RouteApply = 3,
    RouteCleanup = 4,
    DnsApply = 5,
    DnsCleanup = 6,
    FirewallRules = 7,
    KillSwitch = 8
};

class CapabilitySet {
public:
    void add(Capability cap);
    bool has(Capability cap) const;
    std::vector<std::string> to_strings() const;
    static CapabilitySet from_strings(const std::vector<std::string>& caps);

    // Default capabilities per platform
    static CapabilitySet default_for_current_platform();

private:
    std::set<Capability> caps_;
};

} // namespace exv::helper
