#pragma once
// Legacy VPN adapter — encapsulates the legacy openconnect supervisor
// path that is being phased out in favor of the Core-owned native
// engine path via TunnelController.
//
// New code should use TunnelController directly.  This adapter exists
// solely to keep the legacy CLI path (exv start/stop/status) working
// during the migration period.

#include "config.hpp"
#include <string>

namespace ecnuvpn {
namespace vpn {
namespace legacy {

/// Run the legacy openconnect VPN start path.
/// Returns exit code (0 = success).
int start(const Config &cfg, const std::string &plaintext_password,
          int retry_limit);

/// Stop the legacy VPN session.
/// Returns exit code (0 = success).
int stop();

/// Show legacy VPN status.
/// Returns exit code (0 = success).
int status();

} // namespace legacy
} // namespace vpn
} // namespace ecnuvpn
