#pragma once

#include "config.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace vpn {

struct RuntimeStatusSnapshot {
    bool running = false;
    int pid = -1;
    int supervisor_pid = -1;
    bool network_ready = false;
    std::string interface_name;
    std::string internal_ip;
    std::string interfaces_output;
};

RuntimeStatusSnapshot read_runtime_status_snapshot();
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

// Direct-mode API: structured JSON variants for desktop RPC.
// These bypass the helper daemon and read/act on state directly.
nlohmann::json direct_status_json(const Config &cfg);
nlohmann::json direct_stop_json(const Config &cfg);
nlohmann::json direct_start_json(const Config &cfg,
                                 const std::string &plaintext_password,
                                 int retry_limit = 0);

} // namespace vpn
} // namespace ecnuvpn
