#pragma once

#include "core/config/config.hpp"

#include <functional>
#include <string>

namespace ecnuvpn {
namespace vpn {

inline constexpr int kVpnInitialConnectFailedExitCode = 2;

struct RuntimeStatusSnapshot {
    bool running = false;
    int pid = -1;
    bool network_ready = false;
    std::string interface_name;
    std::string internal_ip;
    std::string interfaces_output;
};

struct RuntimeStatusProbe {
    std::function<std::string()> interfaces_output;
};

RuntimeStatusSnapshot read_runtime_status_snapshot();
RuntimeStatusSnapshot read_runtime_status_snapshot(const Config &cfg);
RuntimeStatusSnapshot read_runtime_status_snapshot(const Config &cfg,
                                                   const RuntimeStatusProbe &probe);

// Start the VPN connection via TunnelController
int start(const Config &cfg, int retry_limit = 0);

// Stop the VPN connection
int stop();

// Show VPN status
int status();

} // namespace vpn
} // namespace ecnuvpn
