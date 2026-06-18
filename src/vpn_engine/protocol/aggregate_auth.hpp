#pragma once

#include "vpn_engine/engine.hpp"

#include <string>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

enum class AggregateAuthResponseType {
  unknown,
  auth_request,
  success,
  error,
  challenge,
  group_select,
  host_scan,
};

struct AggregateAuthInitRequest {
  std::string server_url;
  std::string device_id;
  std::string version = "ECNUVPN-NATIVE";
};

struct AggregateAuthReplyRequest {
  std::string username;
  std::string password;
  std::string selected_group;
  std::string challenge_value;
  std::vector<std::string> opaque_xml;
};

struct AggregateAuthChoice {
  std::string value;
  std::string label;
};

struct AggregateAuthField {
  std::string name;
  std::string type;
  std::string label;
  std::string value;
  std::vector<AggregateAuthChoice> options;
};

struct AggregateAuthHostScan {
  std::string ticket;
  std::string token;
  std::string base_uri;
  std::string wait_uri;
};

struct AggregateAuthResponse {
  AggregateAuthResponseType type = AggregateAuthResponseType::unknown;
  std::string auth_id;
  std::string message;
  std::vector<AggregateAuthField> fields;
  std::vector<std::string> opaque_xml;
  AggregateAuthHostScan host_scan;
  std::string session_token;
  std::string session_id;
  std::string session_cookie;
  std::string error_code;
  std::string error_message;
};

std::string build_aggregate_auth_init_xml(
    const AggregateAuthInitRequest &request);

std::string build_aggregate_auth_reply_xml(
    const AggregateAuthReplyRequest &request);

ValidationResult parse_aggregate_auth_response(const std::string &xml,
                                               AggregateAuthResponse *out);

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
