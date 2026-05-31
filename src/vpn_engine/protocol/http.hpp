#pragma once

#include "vpn_engine/engine.hpp"

#include <map>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

struct HttpResponse {
  int status = 0;

  // Lower-cased header name (ASCII) -> flattened value (duplicates joined with ", ").
  std::map<std::string, std::string> headers;

  // Lower-cased header name (ASCII) -> all received values (preserves duplicates).
  std::map<std::string, std::vector<std::string>> header_values;

  std::string body;

  // Case-insensitive header lookup (ASCII).
  const std::string *header_ci(const std::string &name) const;

  // Case-insensitive header lookup (ASCII), preserving duplicates.
  const std::vector<std::string> *header_values_ci(const std::string &name) const;
};

ValidationResult parse_http_response(const std::string &raw, HttpResponse *out);

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
