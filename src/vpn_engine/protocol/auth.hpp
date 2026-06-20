#pragma once

#include "vpn_engine/engine.hpp"
#include "vpn_engine/protocol/http.hpp"

#include <map>
#include <string>
#include <vector>

namespace exv {
namespace vpn_engine {
namespace protocol {

struct AuthForm {
  std::string action_path;
  std::string username_field;
  std::string password_field;
  std::map<std::string, std::string> hidden_fields;
};

struct AuthResult {
  bool ok = false;
  std::string cookie;
  std::string error_code;
  std::string error_message;
  std::string interaction_prompt_label;
  std::string interaction_prompt_type;
  std::string interaction_group_options;
};

class AuthCookieJar {
public:
  void clear();
  void collect_from_response(const HttpResponse &response);

  bool empty() const;
  const std::string &header() const;

private:
  struct Cookie {
    std::string name;
    std::string value;
  };

  void rebuild_header();

  std::vector<Cookie> cookies_;
  std::string header_;
};

ValidationResult parse_auth_form(const std::string &html, AuthForm *out);

AuthResult parse_auth_response(const HttpResponse &response);

} // namespace protocol
} // namespace vpn_engine
} // namespace exv
