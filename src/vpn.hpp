#pragma once

#include "config.hpp"

namespace ecnuvpn {
namespace vpn {

// Start the VPN connection using openconnect
int start(const Config &cfg, int retry_limit = 0);
int start_with_password(const Config &cfg, const std::string &plaintext_password,
                        int retry_limit = 0);

// Stop the VPN connection
int stop();

// Show VPN status
int status();

} // namespace vpn
} // namespace ecnuvpn
