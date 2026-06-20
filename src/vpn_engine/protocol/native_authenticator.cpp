#include "vpn_engine/protocol/native_authenticator.hpp"

#include <utility>

namespace exv {
namespace vpn_engine {
namespace protocol {

namespace {

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

} // namespace

NativeAuthenticator::NativeAuthenticator(ProtocolTransport *transport)
    : transport_(transport) {}

ValidationResult
NativeAuthenticator::authenticate(const NativeAuthRequest &request,
                                  NativeAuthSession *session) {
  if (!session)
    return invalid("auth_session_missing",
                   "native auth session output must not be null");

  *session = NativeAuthSession{};

  if (!transport_)
    return invalid("transport_missing", "protocol transport is not configured");

  AuthResult auth = transport_->authenticate(request.options);
  if (!auth.ok)
    return invalid(auth.error_code, auth.error_message);

  session->server = request.options.server;
  session->username = request.options.username;
  session->useragent = request.options.useragent;
  session->cookie_header = std::move(auth.cookie);
  session->auth_method = "password";
  session->created_at = std::chrono::system_clock::now();
  session->diagnostics["auth_method"] = session->auth_method;
  session->diagnostics["cookie_present"] =
      session->cookie_header.empty() ? "false" : "true";

  return ValidationResult{};
}

} // namespace protocol
} // namespace vpn_engine
} // namespace exv
