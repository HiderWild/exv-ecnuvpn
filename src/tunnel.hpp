#pragma once

#include "config.hpp"
#include <string>

namespace ecnuvpn {
namespace tunnel {

// Generate a platform-specific tunnel helper script based on config routes.
std::string generate(const Config &cfg);

// Write the platform-specific tunnel helper script to the config directory.
bool write_script(const Config &cfg);

// Delete all VPN split-tunnel routes from the OS routing table.
// Uses the route-ready file for interface name and config for route list.
// Safe to call even if routes were already removed (errors are suppressed).
void cleanup_routes();

} // namespace tunnel
} // namespace ecnuvpn
