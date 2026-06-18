#pragma once

#include "vpn_engine/native_engine.hpp"

#include <memory>
#include <stop_token>

namespace ecnuvpn {
namespace vpn_engine {

struct NativeHandshakeResult {
  TunnelMetadata metadata;
  std::unique_ptr<protocol::ProtocolTransport> transport;
  std::unique_ptr<protocol::ProtocolSession> session;
};

class NativeHandshakeJob {
public:
  NativeHandshakeJob(VpnEngineConfig config,
                     NativeVpnEngineDependencies dependencies);

  ValidationResult run(std::stop_token stop, NativeHandshakeResult *out);

private:
  void emit_event(std::string type, std::string level, std::string message,
                  std::map<std::string, std::string> fields = {});

  VpnEngineConfig config_;
  NativeVpnEngineDependencies dependencies_;
};

} // namespace vpn_engine
} // namespace ecnuvpn
