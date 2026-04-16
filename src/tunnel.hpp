#pragma once

#include "config.hpp"
#include <string>

namespace ecnuvpn {
namespace tunnel {

// Generate tunnel.sh script based on config routes
std::string generate(const Config &cfg);

// Write tunnel.sh to ~/.ecnuvpn/tunnel.sh and set executable
bool write_script(const Config &cfg);

} // namespace tunnel
} // namespace ecnuvpn
