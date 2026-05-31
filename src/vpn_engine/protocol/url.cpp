#include "vpn_engine/protocol/url.hpp"

#include <cctype>
#include <string_view>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

namespace {

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

bool is_ascii_space(unsigned char ch) {
  return ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n';
}

std::string_view trim_ascii(std::string_view s) {
  while (!s.empty() && is_ascii_space(static_cast<unsigned char>(s.front())))
    s.remove_prefix(1);
  while (!s.empty() && is_ascii_space(static_cast<unsigned char>(s.back())))
    s.remove_suffix(1);
  return s;
}

std::string to_ascii_lower(std::string_view s) {
  std::string out;
  out.reserve(s.size());
  for (unsigned char ch : s)
    out.push_back(static_cast<char>(std::tolower(ch)));
  return out;
}

bool contains_ascii_space(std::string_view s) {
  for (unsigned char ch : s) {
    if (is_ascii_space(ch))
      return true;
  }
  return false;
}

bool parse_port(std::string_view s, int *out_port) {
  if (!out_port || s.empty())
    return false;

  int value = 0;
  for (unsigned char ch : s) {
    if (ch < '0' || ch > '9')
      return false;
    value = value * 10 + (ch - '0');
    if (value > 65535)
      return false;
  }

  if (value <= 0)
    return false;
  *out_port = value;
  return true;
}

} // namespace

ValidationResult parse_vpn_url(const std::string &input, ParsedVpnUrl *out) {
  if (!out)
    return invalid("url_invalid", "output pointer is null");

  std::string_view s = trim_ascii(input);
  if (s.empty())
    return invalid("url_invalid", "URL is empty");

  std::string scheme = "https";
  std::string_view rest = s;

  const std::size_t scheme_pos = s.find("://");
  if (scheme_pos != std::string_view::npos) {
    scheme = to_ascii_lower(s.substr(0, scheme_pos));
    rest = s.substr(scheme_pos + 3);
  }

  if (scheme != "https")
    return invalid("url_scheme_invalid", "only https:// URLs are supported");

  rest = trim_ascii(rest);
  if (rest.empty() || rest.front() == '/')
    return invalid("url_invalid", "URL host is empty");

  const std::size_t authority_end = rest.find_first_of("/?#");
  std::string_view authority =
      (authority_end == std::string_view::npos) ? rest : rest.substr(0, authority_end);
  std::string_view after_authority =
      (authority_end == std::string_view::npos) ? std::string_view() : rest.substr(authority_end);

  authority = trim_ascii(authority);
  if (authority.empty())
    return invalid("url_invalid", "URL host is empty");
  if (contains_ascii_space(authority))
    return invalid("url_invalid", "URL host contains whitespace");
  if (authority.find('@') != std::string_view::npos)
    return invalid("url_invalid", "userinfo is not supported in VPN URL");

  std::string host;
  int port = 443;

  if (authority.front() == '[') {
    const std::size_t close = authority.find(']');
    if (close == std::string_view::npos)
      return invalid("url_invalid", "unterminated IPv6 host");

    std::string_view host_part = authority.substr(1, close - 1);
    if (host_part.empty())
      return invalid("url_invalid", "URL host is empty");

    host.assign(host_part.begin(), host_part.end());

    std::string_view remainder = authority.substr(close + 1);
    if (!remainder.empty()) {
      if (remainder.front() != ':')
        return invalid("url_invalid", "unexpected characters after IPv6 host");
      if (!parse_port(remainder.substr(1), &port))
        return invalid("url_invalid", "invalid port");
    }
  } else {
    const std::size_t first_colon = authority.find(':');
    const std::size_t last_colon = authority.rfind(':');
    if (first_colon != std::string_view::npos && first_colon != last_colon) {
      return invalid("url_invalid",
                     "IPv6 literals must be bracketed (e.g. https://[::1]/)");
    }

    std::string_view host_part = authority;
    std::string_view port_part;

    if (first_colon != std::string_view::npos) {
      host_part = authority.substr(0, first_colon);
      port_part = authority.substr(first_colon + 1);
      if (port_part.empty())
        return invalid("url_invalid", "port is empty");
      if (!parse_port(port_part, &port))
        return invalid("url_invalid", "invalid port");
    }

    host_part = trim_ascii(host_part);
    if (host_part.empty())
      return invalid("url_invalid", "URL host is empty");
    host.assign(host_part.begin(), host_part.end());
  }

  std::string base_path = "/";
  if (!after_authority.empty()) {
    if (after_authority.front() == '/') {
      const std::size_t end = after_authority.find_first_of("?#");
      std::string_view path_part = (end == std::string_view::npos)
                                       ? after_authority
                                       : after_authority.substr(0, end);
      base_path.assign(path_part.begin(), path_part.end());
      if (base_path.empty())
        base_path = "/";
    } else if (after_authority.front() == '?' || after_authority.front() == '#') {
      base_path = "/";
    } else {
      return invalid("url_invalid", "unexpected characters after host");
    }
  }

  ParsedVpnUrl parsed;
  parsed.scheme = "https";
  parsed.host = std::move(host);
  parsed.port = port;
  parsed.base_path = std::move(base_path);

  *out = std::move(parsed);
  return ValidationResult{};
}

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
