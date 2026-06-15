#pragma once

#include <nlohmann/json.hpp>
#include <ctime>
#include <string>

namespace ecnuvpn {

// Unified error type strings matching the TypeScript VpnErrorType enum.
static constexpr const char *kErrorElevationRequired = "elevation_required";
static constexpr const char *kErrorElevationCancelled = "elevation_cancelled";
static constexpr const char *kErrorElevationDenied    = "elevation_denied";
static constexpr const char *kErrorRuntimeMissing     = "runtime_missing";
static constexpr const char *kErrorConfigInvalid      = "config_invalid";
static constexpr const char *kErrorServiceMissing     = "service_missing";
static constexpr const char *kErrorNativeFailure      = "native_failure";
static constexpr const char *kErrorParseFailure       = "parse_failure";
static constexpr const char *kErrorUnknownAction      = "unknown_action";

inline nlohmann::json structured_error(const char *error_type, const std::string &message,
                                       bool recoverable = true,
                                       const std::string &recommended_action = "") {
  return nlohmann::json{{"ok", false},
                        {"error_type", error_type},
                        {"message", message},
                        {"recoverable", recoverable},
                        {"recommended_action", recommended_action},
                        {"timestamp", static_cast<int64_t>(std::time(nullptr))}};
}

} // namespace ecnuvpn
