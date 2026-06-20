#pragma once

#include "vpn_engine/engine.hpp"

#include <string>

namespace exv {
namespace vpn_engine {
namespace protocol {

struct ParsedVpnUrl {
  std::string scheme;
  std::string host;
  int port = 443;
  std::string base_path = "/";
};

ValidationResult parse_vpn_url(const std::string &input, ParsedVpnUrl *out);

} // namespace protocol
} // namespace vpn_engine
} // namespace exv
