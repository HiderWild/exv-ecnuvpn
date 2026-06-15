#include "helper_capabilities.hpp"
#include <map>

namespace exv::helper {

static const std::map<Capability, std::string> capability_strings = {
    {Capability::TunnelDeviceCreate, "tunnel_device_create"},
    {Capability::TunnelDeviceOpen,   "tunnel_device_open"},
    {Capability::RouteApply,         "route_apply"},
    {Capability::RouteCleanup,       "route_cleanup"},
    {Capability::DnsApply,           "dns_apply"},
    {Capability::DnsCleanup,         "dns_cleanup"},
    {Capability::FirewallRules,      "firewall_rules"},
    {Capability::KillSwitch,         "kill_switch"}
};

void CapabilitySet::add(Capability cap) {
    caps_.insert(cap);
}

bool CapabilitySet::has(Capability cap) const {
    return caps_.count(cap) > 0;
}

std::vector<std::string> CapabilitySet::to_strings() const {
    std::vector<std::string> result;
    for (auto cap : caps_) {
        auto it = capability_strings.find(cap);
        if (it != capability_strings.end()) {
            result.push_back(it->second);
        }
    }
    return result;
}

CapabilitySet CapabilitySet::from_strings(const std::vector<std::string>& caps) {
    CapabilitySet set;
    // Build reverse map
    std::map<std::string, Capability> reverse;
    for (auto& [cap, str] : capability_strings) {
        reverse[str] = cap;
    }
    for (auto& s : caps) {
        auto it = reverse.find(s);
        if (it != reverse.end()) {
            set.caps_.insert(it->second);
        }
    }
    return set;
}

CapabilitySet CapabilitySet::default_for_current_platform() {
    CapabilitySet set;
    set.add(Capability::TunnelDeviceCreate);
    set.add(Capability::TunnelDeviceOpen);
    set.add(Capability::RouteApply);
    set.add(Capability::RouteCleanup);
    set.add(Capability::DnsApply);
    set.add(Capability::DnsCleanup);
    set.add(Capability::FirewallRules);
    set.add(Capability::KillSwitch);
    return set;
}

} // namespace exv::helper
