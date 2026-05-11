#pragma once

#include "config.hpp"
#include <string>

namespace ecnuvpn {
namespace tunnel {

// Generate a platform-specific tunnel helper script based on config routes.
std::string generate(const Config &cfg);

// Write the platform-specific tunnel helper script to the config directory.
bool write_script(const Config &cfg);

} // namespace tunnel
} // namespace ecnuvpn
