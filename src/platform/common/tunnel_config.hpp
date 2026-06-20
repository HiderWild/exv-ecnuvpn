#pragma once
#include <string>
#include <vector>
#include <optional>

namespace exv::platform {

struct RouteEntry {
    std::string destination;   // e.g. "0.0.0.0/0" or "10.0.0.0/8"
    std::string gateway;       // e.g. "10.0.0.1"
    int metric = 0;
    bool is_default = false;
};

struct DnsConfig {
    std::vector<std::string> servers;
    std::string search_domain;
    std::vector<std::string> suffixes;
};

struct FirewallRule {
    std::string name;
    std::string direction;  // "in" or "out"
    std::string action;     // "allow" or "block"
    std::string program_path;  // Optional: apply to specific program
    int priority = 0;
};

struct TunnelConfig {
    std::string interface_address;  // e.g. "10.0.0.2/24"
    std::string interface_name;     // e.g. "tun0" or "EXV"
    int mtu = 1400;
    std::vector<RouteEntry> routes;
    std::vector<std::string> server_bypass_ips;
    DnsConfig dns;
    std::vector<std::string> exclude_routes;
    std::vector<FirewallRule> firewall_rules;
    bool enable_kill_switch = false;
    std::optional<std::string> exclude_route;  // Route to exclude (e.g. VPN server)
};

} // namespace exv::platform
