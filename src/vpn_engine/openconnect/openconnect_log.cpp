#include "vpn_engine/openconnect/openconnect_log.hpp"

#include <algorithm>
#include <cctype>
#include <regex>
#include <string>

namespace ecnuvpn {
namespace openconnect_log {
namespace {

std::string lower_ascii(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
    return static_cast<char>(std::tolower(c));
  });
  return value;
}

std::string latest_attempt_segment(const std::string &content) {
  std::size_t start = content.rfind("Starting VPN:");
  if (start == std::string::npos)
    return content;
  return content.substr(start);
}

} // namespace

bool contains_auth_failure_text(const std::string &text) {
  std::string lower = lower_ascii(text);
  const char *tokens[] = {
      "authentication failed",
      "login failed",
      "login denied",
      "invalid password",
      "password is incorrect",
      "incorrect password",
      "wrong password",
      "bad password",
      "failed to complete authentication",
      "failed to obtain webvpn cookie",
      "server rejected the connection",
      "server rejected connection",
      "xml response has no \"auth\" node",
      "xml response has no 'auth' node",
      "auth failed",
      "auth_failed",
  };
  for (const char *token : tokens) {
    if (lower.find(token) != std::string::npos)
      return true;
  }
  return false;
}

Evidence parse_evidence(const std::string &content) {
  Evidence evidence;
  std::string segment = latest_attempt_segment(content);
  evidence.auth_failed = contains_auth_failure_text(segment);

  std::regex ip_regex(R"(Configured as ([0-9.]+), with)",
                      std::regex_constants::icase);
  std::regex adapter_regex(
      R"(Using [A-Za-z0-9_-]+ device '([^']+)', index ([0-9]+))",
      std::regex_constants::icase);
  std::smatch ip_match;
  std::smatch adapter_match;
  if (std::regex_search(segment, ip_match, ip_regex) &&
      std::regex_search(segment, adapter_match, adapter_regex) &&
      ip_match.size() >= 2 && adapter_match.size() >= 3) {
    evidence.has_tunnel_metadata = true;
    evidence.internal_ip = ip_match[1].str();
    evidence.adapter = adapter_match[1].str();
    evidence.if_index = adapter_match[2].str();
  }

  return evidence;
}

} // namespace openconnect_log
} // namespace ecnuvpn
