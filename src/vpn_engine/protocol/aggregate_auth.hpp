#pragma once

#include "vpn_engine/protocol/http.hpp"

#include <string>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

struct AggregateAuthPrompt {
  std::string kind;
  std::string label;
  std::string input_type;
  std::vector<std::string> options;
};

struct AggregateAuthResult {
  bool ok = false;
  std::string cookie;
  std::string error_code;
  std::string error_message;
  AggregateAuthPrompt prompt;
};

std::string make_aggregate_auth_init_xml();
std::string make_aggregate_auth_reply_xml(const std::string &username,
                                          const std::string &password,
                                          const std::string &group = {},
                                          const std::string &secondary = {});

AggregateAuthResult parse_aggregate_auth_response(const HttpResponse &response);

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
