#include "vpn_engine/protocol/aggregate_auth.hpp"

#include <algorithm>
#include <cctype>
#include <sstream>
#include <string_view>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

namespace {

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

bool starts_with_ci(std::string_view value, std::string_view prefix) {
  if (value.size() < prefix.size())
    return false;
  for (std::size_t i = 0; i < prefix.size(); ++i) {
    const unsigned char a = static_cast<unsigned char>(value[i]);
    const unsigned char b = static_cast<unsigned char>(prefix[i]);
    if (std::tolower(a) != std::tolower(b))
      return false;
  }
  return true;
}

std::string extract_cookie_pair(std::string_view set_cookie_value) {
  const std::size_t semi = set_cookie_value.find(';');
  std::string_view pair = semi == std::string_view::npos
                              ? set_cookie_value
                              : set_cookie_value.substr(0, semi);
  pair = trim_ascii(pair);
  return std::string(pair);
}

std::string webvpn_cookie_from_headers(const HttpResponse &response) {
  const auto *values = response.header_values_ci("set-cookie");
  if (!values)
    return {};

  for (const std::string &raw : *values) {
    const std::string pair = extract_cookie_pair(raw);
    if (starts_with_ci(pair, "webvpn="))
      return pair;
  }
  return {};
}

std::string xml_escape(std::string_view value) {
  std::string out;
  out.reserve(value.size());
  for (const char ch : value) {
    switch (ch) {
    case '&':
      out += "&amp;";
      break;
    case '<':
      out += "&lt;";
      break;
    case '>':
      out += "&gt;";
      break;
    case '"':
      out += "&quot;";
      break;
    case '\'':
      out += "&apos;";
      break;
    default:
      out.push_back(ch);
      break;
    }
  }
  return out;
}

std::string xml_unescape(std::string value) {
  auto replace_all = [](std::string *text, const std::string &needle,
                        const std::string &replacement) {
    std::size_t pos = 0;
    while ((pos = text->find(needle, pos)) != std::string::npos) {
      text->replace(pos, needle.size(), replacement);
      pos += replacement.size();
    }
  };

  replace_all(&value, "&lt;", "<");
  replace_all(&value, "&gt;", ">");
  replace_all(&value, "&quot;", "\"");
  replace_all(&value, "&apos;", "'");
  replace_all(&value, "&amp;", "&");
  return value;
}

std::size_t find_ci(std::string_view haystack, std::string_view needle,
                    std::size_t start = 0) {
  if (needle.empty())
    return start;
  if (start > haystack.size() || needle.size() > haystack.size())
    return std::string_view::npos;

  for (std::size_t i = start; i + needle.size() <= haystack.size(); ++i) {
    bool match = true;
    for (std::size_t j = 0; j < needle.size(); ++j) {
      const unsigned char a = static_cast<unsigned char>(haystack[i + j]);
      const unsigned char b = static_cast<unsigned char>(needle[j]);
      if (std::tolower(a) != std::tolower(b)) {
        match = false;
        break;
      }
    }
    if (match)
      return i;
  }
  return std::string_view::npos;
}

std::string tag_text(std::string_view body, std::string_view tag) {
  const std::string open = "<" + std::string(tag);
  const std::string close = "</" + std::string(tag) + ">";
  const std::size_t open_pos = find_ci(body, open);
  if (open_pos == std::string_view::npos)
    return {};

  const std::size_t open_end = body.find('>', open_pos);
  if (open_end == std::string_view::npos)
    return {};

  const std::size_t close_pos = find_ci(body, close, open_end + 1);
  if (close_pos == std::string_view::npos || close_pos < open_end + 1)
    return {};

  std::string_view text = body.substr(open_end + 1, close_pos - (open_end + 1));
  text = trim_ascii(text);
  return xml_unescape(std::string(text));
}

std::vector<std::string> option_values(std::string_view body) {
  std::vector<std::string> values;
  std::size_t pos = 0;
  while ((pos = find_ci(body, "<option", pos)) != std::string_view::npos) {
    const std::size_t tag_end = body.find('>', pos);
    if (tag_end == std::string_view::npos)
      break;
    const std::string_view tag = body.substr(pos, tag_end - pos);
    const std::size_t value_pos = find_ci(tag, "value=");
    if (value_pos != std::string_view::npos) {
      const std::size_t quote_pos = value_pos + 6;
      if (quote_pos < tag.size() && (tag[quote_pos] == '"' || tag[quote_pos] == '\'')) {
        const char quote = tag[quote_pos];
        const std::size_t value_start = quote_pos + 1;
        const std::size_t value_end = tag.find(quote, value_start);
        if (value_end != std::string_view::npos) {
          values.push_back(xml_unescape(std::string(
              tag.substr(value_start, value_end - value_start))));
        }
      }
    }
    pos = tag_end + 1;
  }
  return values;
}

AggregateAuthResult error(std::string code, std::string message) {
  AggregateAuthResult result;
  result.ok = false;
  result.error_code = std::move(code);
  result.error_message = std::move(message);
  return result;
}

std::string attribute_value(std::string_view tag, std::string_view name) {
  std::size_t pos = 0;
  while ((pos = find_ci(tag, name, pos)) != std::string_view::npos) {
    const bool left_ok =
        pos == 0 || is_ascii_space(static_cast<unsigned char>(tag[pos - 1]));
    const std::size_t after_name = pos + name.size();
    if (!left_ok || after_name >= tag.size()) {
      pos = after_name;
      continue;
    }

    std::size_t cursor = after_name;
    while (cursor < tag.size() &&
           is_ascii_space(static_cast<unsigned char>(tag[cursor]))) {
      ++cursor;
    }
    if (cursor >= tag.size() || tag[cursor] != '=') {
      pos = after_name;
      continue;
    }
    ++cursor;
    while (cursor < tag.size() &&
           is_ascii_space(static_cast<unsigned char>(tag[cursor]))) {
      ++cursor;
    }
    if (cursor >= tag.size() || (tag[cursor] != '"' && tag[cursor] != '\'')) {
      pos = after_name;
      continue;
    }

    const char quote = tag[cursor++];
    const std::size_t value_end = tag.find(quote, cursor);
    if (value_end == std::string_view::npos)
      return {};
    return xml_unescape(std::string(tag.substr(cursor, value_end - cursor)));
  }
  return {};
}

struct AuthInput {
  std::string name;
  std::string type;
  std::string label;
};

std::vector<AuthInput> input_fields(std::string_view body) {
  std::vector<AuthInput> fields;
  std::size_t pos = 0;
  while ((pos = find_ci(body, "<input", pos)) != std::string_view::npos) {
    const std::size_t tag_end = body.find('>', pos);
    if (tag_end == std::string_view::npos)
      break;

    const std::string_view tag = body.substr(pos, tag_end - pos);
    AuthInput field;
    field.name = attribute_value(tag, "name");
    field.type = attribute_value(tag, "type");
    field.label = attribute_value(tag, "label");
    if (field.type.empty())
      field.type = "text";
    if (!field.name.empty())
      fields.push_back(std::move(field));
    pos = tag_end + 1;
  }
  return fields;
}

bool is_challenge_input(const AuthInput &field) {
  const std::string name = to_ascii_lower(field.name);
  return name != "username" && name != "group";
}

AggregateAuthResult challenge_from_inputs(const std::vector<AuthInput> &fields) {
  for (const AuthInput &field : fields) {
    if (!is_challenge_input(field))
      continue;

    auto result = error("auth_challenge_required",
                        "authentication challenge response is required");
    result.prompt.kind = "challenge";
    result.prompt.label =
        field.label.empty() ? "Authentication challenge" : field.label;
    result.prompt.input_type = field.type.empty() ? "text" : field.type;
    return result;
  }
  return {};
}

} // namespace

std::string make_aggregate_auth_init_xml() {
  return "<?xml version=\"1.0\" encoding=\"UTF-8\"?>"
         "<config-auth client=\"vpn\" type=\"init\">"
         "<version who=\"vpn\">4.10</version>"
         "</config-auth>";
}

std::string make_aggregate_auth_reply_xml(const std::string &username,
                                          const std::string &password,
                                          const std::string &group,
                                          const std::string &secondary) {
  std::ostringstream out;
  out << "<?xml version=\"1.0\" encoding=\"UTF-8\"?>"
      << "<config-auth client=\"vpn\" type=\"auth-reply\">"
      << "<auth>"
      << "<username>" << xml_escape(username) << "</username>"
      << "<password>" << xml_escape(password) << "</password>";
  if (!group.empty())
    out << "<group>" << xml_escape(group) << "</group>";
  if (!secondary.empty())
    out << "<secondary_password>" << xml_escape(secondary)
        << "</secondary_password>";
  out << "</auth></config-auth>";
  return out.str();
}

AggregateAuthResult parse_aggregate_auth_response(const HttpResponse &response) {
  if (response.status == 401 || response.status == 403) {
    return error("auth_failed", "authentication failed");
  }
  if (response.status < 200 || response.status >= 300) {
    return error("protocol_error", "unexpected HTTP status in auth response");
  }

  const std::string header_cookie = webvpn_cookie_from_headers(response);
  if (!header_cookie.empty()) {
    AggregateAuthResult result;
    result.ok = true;
    result.cookie = header_cookie;
    return result;
  }

  const std::string body_lower = to_ascii_lower(response.body);
  if (body_lower.find("saml") != std::string::npos ||
      body_lower.find("sso") != std::string::npos) {
    return error("saml_required_unsupported",
                 "SAML authentication is required but browser-based SSO is "
                 "not supported by the native engine");
  }

  if (body_lower.find("host-scan") != std::string::npos ||
      body_lower.find("hostscan") != std::string::npos ||
      body_lower.find("<csd") != std::string::npos ||
      body_lower.find(" csd") != std::string::npos) {
    return error("csd_required_unsupported", "AnyConnect host-scan is required");
  }

  const std::string token = tag_text(response.body, "session-token");
  if (!token.empty()) {
    AggregateAuthResult result;
    result.ok = true;
    result.cookie = "webvpn=" + token;
    return result;
  }

  if (body_lower.find("group_list") != std::string::npos ||
      body_lower.find("name=\"group\"") != std::string::npos ||
      body_lower.find("name='group'") != std::string::npos) {
    auto result = error("auth_group_required", "VPN group selection is required");
    result.prompt.kind = "group";
    result.prompt.label = "VPN group";
    result.prompt.input_type = "select";
    result.prompt.options = option_values(response.body);
    return result;
  }

  const std::vector<AuthInput> inputs = input_fields(response.body);
  AggregateAuthResult input_challenge = challenge_from_inputs(inputs);
  if (!input_challenge.error_code.empty())
    return input_challenge;

  if (body_lower.find("secondary_password") != std::string::npos ||
      body_lower.find("tokencode") != std::string::npos ||
      body_lower.find("challenge") != std::string::npos) {
    auto result = error("auth_challenge_required",
                        "authentication challenge response is required");
    result.prompt.kind = "challenge";
    result.prompt.label = "Authentication challenge";
    result.prompt.input_type = "password";
    return result;
  }

  const std::string message = tag_text(response.body, "message");
  if (!message.empty())
    return error("auth_failed", message);

  return error("protocol_error", "missing session token in auth response");
}

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
