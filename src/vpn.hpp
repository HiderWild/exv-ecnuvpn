#pragma once

#include "config.hpp"

#include <functional>
#include <string>

namespace ecnuvpn {
namespace vpn {

inline constexpr int kVpnInitialConnectFailedExitCode = 2;
// Returned when native engine should use TunnelController (Core-owned mode)
// instead of spawning a supervisor process.
inline constexpr int kUseTunnelController = 3;

struct RuntimeStatusSnapshot {
    bool running = false;
    int pid = -1;
    int supervisor_pid = -1;
    bool pid_from_openconnect_scan = false;
    bool network_ready = false;
    std::string interface_name;
    std::string internal_ip;
    std::string interfaces_output;
};

struct RuntimeStatusProbe {
    std::function<bool(int)> is_process_alive;
    std::function<int()> find_openconnect_pid;
    std::function<std::string()> interfaces_output;
};

RuntimeStatusSnapshot read_runtime_status_snapshot();
RuntimeStatusSnapshot read_runtime_status_snapshot(const Config &cfg);
RuntimeStatusSnapshot read_runtime_status_snapshot(const Config &cfg,
                                                   const RuntimeStatusProbe &probe);
bool stop_direct_session();

// Start the VPN connection using openconnect
int start(const Config &cfg, int retry_limit = 0);
int start_with_password(const Config &cfg, const std::string &plaintext_password,
                        int retry_limit = 0);

#ifdef _WIN32
// Hidden entrypoint used for the detached Windows reconnect supervisor.
int supervisor_main();
#endif

// Stop the VPN connection
int stop();

// Show VPN status
int status();

} // namespace vpn
} // namespace ecnuvpn
