#pragma once

#include "vpn_engine/engine.hpp"

#include <cstdint>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

struct TlsEndpoint {
  std::string host;
  int port = 443;
  std::string sni_host;
};

class TlsStream {
public:
  virtual ~TlsStream() = default;

  virtual ValidationResult connect(const TlsEndpoint &endpoint) = 0;
  virtual ValidationResult write_all(const std::vector<std::uint8_t> &bytes) = 0;
  virtual ValidationResult read_some(std::vector<std::uint8_t> *bytes) = 0;
  virtual void close() = 0;
};

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
