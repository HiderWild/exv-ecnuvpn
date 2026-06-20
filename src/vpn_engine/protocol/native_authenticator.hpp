#pragma once

#include "vpn_engine/protocol/session.hpp"

#include <chrono>
#include <map>
#include <string>

namespace exv {
namespace vpn_engine {
namespace protocol {

struct NativeAuthSession {
  ParsedVpnUrl server;
  std::string username;
  std::string useragent;
  std::string cookie_header;
  std::string selected_group;
  std::string auth_method = "password";
  std::chrono::system_clock::time_point created_at{};
  std::map<std::string, std::string> diagnostics;
};

struct NativeAuthRequest {
  ProtocolSessionOptions options;
};

class NativeAuthenticator {
public:
  explicit NativeAuthenticator(ProtocolTransport *transport);

  ValidationResult authenticate(const NativeAuthRequest &request,
                                NativeAuthSession *session);

private:
  ProtocolTransport *transport_ = nullptr;
};

} // namespace protocol
} // namespace vpn_engine
} // namespace exv
