#pragma once

#include "config.hpp"

#include <nlohmann/json.hpp>

#include <string>

namespace ecnuvpn {
namespace platform {

// --- Existing policy helpers ---

void prepare_direct_fallback_runtime();
std::string helper_unavailable_connect_message();
std::string helper_unavailable_disconnect_message();

// --- Platform-specific RPC dispatch ---

// Validate platform-specific preconditions before a VPN connect attempt.
// Returns a JSON error object on failure, or an empty object on success.
// Windows checks driver availability; macOS/Linux return success (no-op).
nlohmann::json preflight_connect_platform_checks(const Config &cfg);

// Attempt a direct (non-helper) VPN connect when the helper is unavailable.
// Returns a non-empty JSON result on the direct-fallback path, or an empty
// object if the platform does not support direct fallback (Windows).
nlohmann::json try_connect_direct_fallback(const Config &cfg,
                                            const std::string &password);

// Attempt a direct (non-helper) VPN disconnect when the helper is unavailable.
// Returns a non-empty JSON result on the direct-fallback path, or an empty
// object if the platform does not support direct fallback (Windows).
nlohmann::json try_disconnect_direct_fallback(bool allow_direct_fallback);

// Build a VPN status response when the helper is unavailable.
// macOS/Linux fall back to the runtime status snapshot; Windows returns
// an empty object (helper is always expected on Windows).
nlohmann::json status_fallback_without_helper(const Config &cfg);

} // namespace platform
} // namespace ecnuvpn