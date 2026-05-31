#pragma once

#include <string>

namespace ecnuvpn {
namespace openconnect_log {

struct Evidence {
  bool auth_failed = false;
  bool has_tunnel_metadata = false;
  std::string internal_ip;
  std::string adapter;
  std::string if_index;
};

bool contains_auth_failure_text(const std::string &text);
Evidence parse_evidence(const std::string &content);

} // namespace openconnect_log
} // namespace ecnuvpn
