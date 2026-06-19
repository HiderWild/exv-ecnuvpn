#include "feedback/feedback.hpp"

#include <array>

namespace ecnuvpn {
namespace feedback {

namespace {

std::string to_lower_ascii(const std::string &value) {
  std::string out;
  out.reserve(value.size());
  for (char c : value) {
    if (c >= 'A' && c <= 'Z')
      c = static_cast<char>(c - 'A' + 'a');
    out.push_back(c);
  }
  return out;
}

bool contains(const std::string &haystack, const char *needle) {
  return haystack.find(needle) != std::string::npos;
}

bool is_canonical(const std::string &value) {
  return value == code::kTlsVerifyFailed || value == code::kWintunMissing ||
         value == code::kUtunPermissionDenied || value == code::kAuthFailed ||
         value == code::kUnsupportedDtls || value == code::kPermissionDenied ||
         value == code::kHelperUnavailable ||
         value == code::kNetworkUnreachable || value == code::kUserCancelled ||
         value == code::kInvalidRequest ||
         value == code::kAuthProtocolMismatch ||
         value == code::kAuthRejected ||
         value == code::kAuthChallengeRequired ||
         value == code::kAuthGroupRequired ||
         value == code::kAuthExpired ||
         value == code::kCsdRequiredUnsupported ||
         value == code::kDtlsUnavailable ||
         value == code::kTunnelDisconnected ||
         value == code::kSessionTimeout ||
         value == code::kIdleTimeout ||
         value == code::kRekeyUnsupported ||
         value == code::kCstpCompressedUnsupported ||
         value == code::kUnsupportedExtraArgs ||
         value == code::kConnectionAttemptActive ||
         value == code::kConnectionFailed;
}

ErrorInfo info_for(const std::string &canonical) {
  if (canonical == code::kTlsVerifyFailed)
    return {canonical, true,
            "Verify the server certificate or your system clock, then retry."};
  if (canonical == code::kWintunMissing)
    return {canonical, true,
            "Install the bundled Wintun driver and retry the connection."};
  if (canonical == code::kUtunPermissionDenied)
    return {canonical, true,
            "Grant the VPN helper administrator/sudo permission and retry."};
  if (canonical == code::kAuthFailed)
    return {canonical, true,
            "Check your username and password, then connect again."};
  if (canonical == code::kUnsupportedDtls)
    return {canonical, false,
            "Disable DTLS for this profile; the native engine uses CSTP only."};
  if (canonical == code::kPermissionDenied)
    return {canonical, true,
            "Approve the administrator/UAC prompt to continue."};
  if (canonical == code::kHelperUnavailable)
    return {canonical, true,
            "Start or reinstall the EXV helper service and retry."};
  if (canonical == code::kNetworkUnreachable)
    return {canonical, true,
            "Check your internet connection or VPN server address."};
  if (canonical == code::kUserCancelled)
    return {canonical, true, ""};
  if (canonical == code::kInvalidRequest)
    return {canonical, false, ""};
  if (canonical == code::kAuthProtocolMismatch)
    return {canonical, false,
            "The VPN gateway returned an unexpected auth response; retry or contact support."};
  if (canonical == code::kAuthRejected)
    return {canonical, true,
            "Check your username and password, then connect again."};
  if (canonical == code::kAuthChallengeRequired)
    return {canonical, true,
            "Complete the VPN verification prompt to continue."};
  if (canonical == code::kAuthGroupRequired)
    return {canonical, true,
            "Choose the required VPN group and connect again."};
  if (canonical == code::kAuthExpired)
    return {canonical, true,
            "The VPN authentication session expired; reconnect and authenticate again."};
  if (canonical == code::kCsdRequiredUnsupported)
    return {canonical, false,
            "This gateway requires host-scan, which the native engine does not support yet."};
  if (canonical == code::kDtlsUnavailable)
    return {canonical, true,
            "Continue with CSTP-only mode or retry the connection."};
  if (canonical == code::kTunnelDisconnected)
    return {canonical, true,
            "The VPN gateway disconnected the tunnel; reconnect if needed."};
  if (canonical == code::kSessionTimeout)
    return {canonical, false,
            "The VPN session expired; reconnect and authenticate again."};
  if (canonical == code::kIdleTimeout)
    return {canonical, true,
            "The VPN session was idle too long; reconnect when needed."};
  if (canonical == code::kRekeyUnsupported)
    return {canonical, true,
            "Reconnect the tunnel; this gateway requested an unsupported rekey mode."};
  if (canonical == code::kCstpCompressedUnsupported)
    return {canonical, false,
            "The gateway enabled CSTP compression, which is not supported yet."};
  if (canonical == code::kUnsupportedExtraArgs)
    return {canonical, true,
            "Remove unsupported native extra_args or use a supported compatibility flag."};
  if (canonical == code::kConnectionAttemptActive)
    return {canonical, true,
            "Wait for the active connection attempt to finish or cancel it, then retry."};
  return {code::kConnectionFailed, true,
          "Open the logs (exv logs) to see the underlying failure, then retry."};
}

} // namespace

std::string resolve_error_code(const std::string &raw_code,
                               const std::string &message) {
  if (is_canonical(raw_code))
    return raw_code;

  // Legacy / internal aliases.
  if (raw_code == "unsupported_auth_flow")
    return code::kAuthFailed;
  if (raw_code == "cancelled" || raw_code == "canceled")
    return code::kUserCancelled;
  if (raw_code == "helper_not_running" || raw_code == "helper_missing")
    return code::kHelperUnavailable;

  const std::string hint = to_lower_ascii(raw_code + " " + message);
  if (hint.find_first_not_of(" ") == std::string::npos)
    return code::kConnectionFailed;

  if (contains(hint, "wintun"))
    return code::kWintunMissing;
  if (contains(hint, "utun") && contains(hint, "permission"))
    return code::kUtunPermissionDenied;
  if (contains(hint, "user cancel") || contains(hint, "用户已取消") ||
      contains(hint, "cancelled by user"))
    return code::kUserCancelled;
  if (contains(hint, "permission denied") || contains(hint, "eperm") ||
      contains(hint, "access is denied") || contains(hint, "elevat") ||
      contains(hint, "administrator"))
    return code::kPermissionDenied;
  if (contains(hint, "certificate") || contains(hint, "tls verify") ||
      contains(hint, "verify failed"))
    return code::kTlsVerifyFailed;
  if (contains(hint, "helper") &&
      (contains(hint, "unavailable") || contains(hint, "not running") ||
       contains(hint, "connect")))
    return code::kHelperUnavailable;
  if (contains(hint, "unreachable") || contains(hint, "no route") ||
      contains(hint, "timed out") || contains(hint, "timeout") ||
      contains(hint, "resolve"))
    return code::kNetworkUnreachable;
  if (contains(hint, "auth") || contains(hint, "password") ||
      contains(hint, "rejected"))
    return code::kAuthFailed;

  return code::kConnectionFailed;
}

ErrorInfo lookup_error(const std::string &raw_code,
                       const std::string &message) {
  return info_for(resolve_error_code(raw_code, message));
}

nlohmann::json make_error(const std::string &message,
                          const std::string &raw_code,
                          const std::string &raw_message) {
  const std::string probe_message =
      raw_message.empty() ? message : raw_message;
  ErrorInfo info = lookup_error(raw_code, probe_message);
  return nlohmann::json{{"ok", false},
                        {"message", message},
                        {"code", info.code},
                        {"recoverable", info.recoverable},
                        {"recommended_action", info.recommended_action}};
}

nlohmann::json normalize_error(nlohmann::json response) {
  if (!response.is_object())
    return response;
  if (response.value("ok", true) != false)
    return response;

  std::string raw_code = response.value("code", std::string());
  std::string message = response.value("message", std::string());
  ErrorInfo info = lookup_error(raw_code, message);
  response["code"] = info.code;
  response["recoverable"] = info.recoverable;
  if (!response.contains("recommended_action") ||
      response["recommended_action"].is_null()) {
    response["recommended_action"] = info.recommended_action;
  }
  return response;
}

} // namespace feedback
} // namespace ecnuvpn
