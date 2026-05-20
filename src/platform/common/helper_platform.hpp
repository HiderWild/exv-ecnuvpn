#pragma once

#include <string>

namespace ecnuvpn {
namespace platform {

// Returns the IPC endpoint path for the helper daemon.
// macOS/Linux: Unix domain socket path
// Windows: Named pipe path
std::string helper_endpoint_path();

// Returns the session state file path.
std::string helper_state_path();

// Returns the stable installation path for the main exv binary.
std::string stable_install_path();

// Returns the stable installation path for the helper binary (Windows only).
// On POSIX platforms, returns the same as stable_install_path().
std::string stable_helper_install_path();

// Returns the service label/name used by the platform's service manager.
// macOS: launchd label (e.g. "com.ecnu.exv.helper")
// Linux: systemd service name (e.g. "exv-helper")
// Windows: service name (e.g. "exv-helper")
std::string helper_service_label();

// Returns the service configuration file path (POSIX only).
// macOS: plist path
// Linux: systemd unit path
// Windows: empty string (SCM doesn't use a config file)
std::string helper_service_config_path();

} // namespace platform
} // namespace ecnuvpn
