#pragma once

#include <string>

namespace ecnuvpn {
namespace vpn_engine {

// Canonical native failure codes surfaced to the desktop RPC contract. Keep
// protocol-specific AnyConnect outcomes stable so the UI and logs can
// distinguish auth interaction, gateway disconnect, timeout, and CSTP feature
// gaps from generic transport failure.
inline constexpr const char *kNativeContractTlsVerifyFailed = "tls_verify_failed";
inline constexpr const char *kNativeContractWintunMissing = "wintun_missing";
inline constexpr const char *kNativeContractUtunPermissionDenied =
    "utun_permission_denied";
inline constexpr const char *kNativeContractAuthFailed = "auth_failed";
inline constexpr const char *kNativeContractUnsupportedDtls = "unsupported_dtls";
inline constexpr const char *kNativeContractAuthProtocolMismatch =
    "auth_protocol_mismatch";
inline constexpr const char *kNativeContractAuthRejected = "auth_rejected";
inline constexpr const char *kNativeContractAuthChallengeRequired =
    "auth_challenge_required";
inline constexpr const char *kNativeContractAuthGroupRequired =
    "auth_group_required";
inline constexpr const char *kNativeContractAuthExpired = "auth_expired";
inline constexpr const char *kNativeContractCsdRequiredUnsupported =
    "csd_required_unsupported";
inline constexpr const char *kNativeContractDtlsUnavailable =
    "dtls_unavailable";
inline constexpr const char *kNativeContractTunnelDisconnected =
    "tunnel_disconnected";
inline constexpr const char *kNativeContractSessionTimeout =
    "session_timeout";
inline constexpr const char *kNativeContractIdleTimeout = "idle_timeout";
inline constexpr const char *kNativeContractRekeyUnsupported =
    "rekey_unsupported";
inline constexpr const char *kNativeContractCstpCompressedUnsupported =
    "cstp_compressed_unsupported";
inline constexpr const char *kNativeContractUnsupportedExtraArgs =
    "unsupported_extra_args";

namespace detail {

inline std::string to_lower_ascii(const std::string &value) {
  std::string out;
  out.reserve(value.size());
  for (char c : value) {
    if (c >= 'A' && c <= 'Z')
      c = static_cast<char>(c - 'A' + 'a');
    out.push_back(c);
  }
  return out;
}

inline bool contains(const std::string &haystack, const char *needle) {
  return haystack.find(needle) != std::string::npos;
}

} // namespace detail

// Map an internal native engine failure code (and its human-readable message)
// onto one of the canonical desktop-contract codes. Internal codes are matched
// first; otherwise the message is inspected for well-known substrings. When no
// mapping is recognized the original (possibly empty) code is returned so the
// frontend can fall back to its generic native-failure handling.
inline std::string
map_native_error_to_contract_code(const std::string &code,
                                  const std::string &message) {
  if (code == kNativeContractTlsVerifyFailed)
    return kNativeContractTlsVerifyFailed;
  if (code == kNativeContractUnsupportedDtls)
    return kNativeContractUnsupportedDtls;
  if (code == kNativeContractAuthProtocolMismatch ||
      code == kNativeContractAuthRejected ||
      code == kNativeContractAuthChallengeRequired ||
      code == kNativeContractAuthGroupRequired ||
      code == kNativeContractAuthExpired ||
      code == kNativeContractCsdRequiredUnsupported ||
      code == kNativeContractDtlsUnavailable ||
      code == kNativeContractTunnelDisconnected ||
      code == kNativeContractSessionTimeout ||
      code == kNativeContractIdleTimeout ||
      code == kNativeContractRekeyUnsupported ||
      code == kNativeContractCstpCompressedUnsupported ||
      code == kNativeContractUnsupportedExtraArgs) {
    return code;
  }
  if (code == kNativeContractWintunMissing)
    return kNativeContractWintunMissing;
  if (code == kNativeContractUtunPermissionDenied)
    return kNativeContractUtunPermissionDenied;
  if (code == kNativeContractAuthFailed || code == "unsupported_auth_flow")
    return kNativeContractAuthFailed;

  const std::string hint = detail::to_lower_ascii(code + " " + message);

  if (detail::contains(hint, "wintun"))
    return kNativeContractWintunMissing;
  if (detail::contains(hint, "utun") &&
      detail::contains(hint, "permission"))
    return kNativeContractUtunPermissionDenied;
  if (detail::contains(hint, "permission denied") ||
      detail::contains(hint, "eperm"))
    return kNativeContractUtunPermissionDenied;
  if (detail::contains(hint, "certificate") ||
      detail::contains(hint, "tls verify") ||
      detail::contains(hint, "verify failed"))
    return kNativeContractTlsVerifyFailed;
  if (detail::contains(hint, "auth"))
    return kNativeContractAuthFailed;

  return code;
}

} // namespace vpn_engine
} // namespace ecnuvpn
