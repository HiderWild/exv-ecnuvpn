#pragma once

#include <nlohmann/json.hpp>

#include <string>

namespace ecnuvpn {
namespace feedback {

// ── Canonical error taxonomy ────────────────────────────────────────────────
// Single source of truth for every error code surfaced to the user, merging the
// native-engine contract codes with the desktop/helper codes. Producers must
// never invent ad-hoc codes; they map their raw code+message through
// resolve_error_code() so the renderer can render one consistent label/action.
namespace code {
inline constexpr const char *kTlsVerifyFailed = "tls_verify_failed";
inline constexpr const char *kWintunMissing = "wintun_missing";
inline constexpr const char *kUtunPermissionDenied = "utun_permission_denied";
inline constexpr const char *kAuthFailed = "auth_failed";
inline constexpr const char *kUnsupportedDtls = "unsupported_dtls";
inline constexpr const char *kPermissionDenied = "permission_denied";
inline constexpr const char *kHelperUnavailable = "helper_unavailable";
inline constexpr const char *kNetworkUnreachable = "network_unreachable";
inline constexpr const char *kUserCancelled = "user_cancelled";
inline constexpr const char *kInvalidRequest = "invalid_request";
inline constexpr const char *kAuthProtocolMismatch = "auth_protocol_mismatch";
inline constexpr const char *kAuthRejected = "auth_rejected";
inline constexpr const char *kAuthChallengeRequired = "auth_challenge_required";
inline constexpr const char *kAuthGroupRequired = "auth_group_required";
inline constexpr const char *kAuthExpired = "auth_expired";
inline constexpr const char *kCsdRequiredUnsupported =
    "csd_required_unsupported";
inline constexpr const char *kDtlsUnavailable = "dtls_unavailable";
inline constexpr const char *kTunnelDisconnected = "tunnel_disconnected";
inline constexpr const char *kSessionTimeout = "session_timeout";
inline constexpr const char *kIdleTimeout = "idle_timeout";
inline constexpr const char *kRekeyUnsupported = "rekey_unsupported";
inline constexpr const char *kCstpCompressedUnsupported =
    "cstp_compressed_unsupported";
inline constexpr const char *kUnsupportedExtraArgs = "unsupported_extra_args";
inline constexpr const char *kConnectionAttemptActive =
    "connection_attempt_active";
// Named generic fallback. INVARIANT: a failed result never carries an empty
// code, so the frontend never has to render a context-free "Failed..." popup.
inline constexpr const char *kConnectionFailed = "connection_failed";
} // namespace code

// Stable, user-facing metadata for an error code.
struct ErrorInfo {
  std::string code;               // canonical code (never empty)
  bool recoverable = true;        // can the user retry after acting?
  std::string recommended_action; // short imperative hint, may be empty
};

// Map any raw code (possibly empty) + message onto a canonical code. Never
// returns empty: unrecognized failures collapse to kConnectionFailed.
std::string resolve_error_code(const std::string &raw_code,
                               const std::string &message);

// Look up stable metadata for a (already canonical or raw) code.
ErrorInfo lookup_error(const std::string &raw_code, const std::string &message);

// Build the canonical error envelope shared by helper IPC, desktop-rpc and the
// WebUI HTTP API:
//   { ok:false, code, message, recoverable, recommended_action }
nlohmann::json make_error(const std::string &message,
                          const std::string &raw_code = std::string(),
                          const std::string &raw_message = std::string());

// Normalize an already-built error-ish JSON so it always carries a non-empty
// canonical code plus recoverable/recommended_action. Pass-through for success
// payloads (ok != false).
nlohmann::json normalize_error(nlohmann::json response);

} // namespace feedback
} // namespace ecnuvpn
