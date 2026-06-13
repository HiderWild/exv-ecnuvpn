#pragma once

#include "config.hpp"
#include <string>

namespace ecnuvpn {
namespace tunnel {

// Generate a platform-specific tunnel helper script based on config routes.
std::string generate(const Config &cfg);

// Write the platform-specific tunnel helper script to the config directory.
bool write_script(const Config &cfg);

// Execute the platform-specific openconnect script hook.
int run_script_hook();

// Best-effort fallback for platforms where openconnect cannot launch its
// configured script hook reliably.
bool configure_from_runtime_log(const Config &cfg);

// Delete all VPN split-tunnel routes from the OS routing table.
void cleanup_routes();

} // namespace tunnel
} // namespace ecnuvpn
