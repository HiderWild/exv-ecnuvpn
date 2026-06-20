#include "vpn_engine/protocol/auth.hpp"

#include <algorithm>
#include <cctype>
#include <string_view>
#include <utility>

namespace exv {
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

bool iequals_ascii(std::string_view a, std::string_view b) {
  if (a.size() != b.size())
    return false;
  for (std::size_t i = 0; i < a.size(); ++i) {
    unsigned char ca = static_cast<unsigned char>(a[i]);
    unsigned char cb = static_cast<unsigned char>(b[i]);
    if (std::tolower(ca) != std::tolower(cb))
      return false;
  }
  return true;
}

bool icontains_ascii(std::string_view haystack, std::string_view needle) {
  if (needle.empty())
    return true;
  if (needle.size() > haystack.size())
    return false;

  for (std::size_t i = 0; i + needle.size() <= haystack.size(); ++i) {
    bool match = true;
    for (std::size_t j = 0; j < needle.size(); ++j) {
      unsigned char a = static_cast<unsigned char>(haystack[i + j]);
      unsigned char b = static_cast<unsigned char>(needle[j]);
      if (std::tolower(a) != std::tolower(b)) {
        match = false;
        break;
      }
    }
    if (match)
      return true;
  }
  return false;
}

std::size_t ifind_ascii(std::string_view haystack, std::string_view needle,
                        std::size_t start = 0) {
  if (needle.empty())
    return start;
  if (start > haystack.size())
    return std::string_view::npos;

  for (std::size_t i = start; i + needle.size() <= haystack.size(); ++i) {
    bool match = true;
    for (std::size_t j = 0; j < needle.size(); ++j) {
      unsigned char a = static_cast<unsigned char>(haystack[i + j]);
      unsigned char b = static_cast<unsigned char>(needle[j]);
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

struct AttrParseResult {
  std::string name;
  std::string value;
  std::size_t next = 0;
};

bool is_name_char(unsigned char ch) {
  return std::isalnum(ch) || ch == '_' || ch == '-' || ch == ':';
}

AttrParseResult parse_one_attr(std::string_view s, std::size_t pos) {
  AttrParseResult out;
  out.next = pos;

  while (out.next < s.size() && is_ascii_space(static_cast<unsigned char>(s[out.next])))
    ++out.next;
  if (out.next >= s.size())
    return out;

  const std::size_t name_start = out.next;
  while (out.next < s.size() && is_name_char(static_cast<unsigned char>(s[out.next])))
    ++out.next;
  if (out.next == name_start)
    return out;

  std::string_view name_sv = s.substr(name_start, out.next - name_start);
  out.name.assign(name_sv.begin(), name_sv.end());

  while (out.next < s.size() && is_ascii_space(static_cast<unsigned char>(s[out.next])))
    ++out.next;
  if (out.next >= s.size() || s[out.next] != '=') {
    out.value = "";
    return out;
  }
  ++out.next;

  while (out.next < s.size() && is_ascii_space(static_cast<unsigned char>(s[out.next])))
    ++out.next;
  if (out.next >= s.size()) {
    out.value = "";
    return out;
  }

  const char quote = s[out.next];
  if (quote == '\'' || quote == '"') {
    ++out.next;
    const std::size_t val_start = out.next;
    while (out.next < s.size() && s[out.next] != quote)
      ++out.next;
    std::string_view val_sv = s.substr(val_start, out.next - val_start);
    out.value.assign(val_sv.begin(), val_sv.end());
    if (out.next < s.size() && s[out.next] == quote)
      ++out.next;
    return out;
  }

  const std::size_t val_start = out.next;
  while (out.next < s.size() && !is_ascii_space(static_cast<unsigned char>(s[out.next])) &&
         s[out.next] != '>') {
    ++out.next;
  }
  std::string_view val_sv = s.substr(val_start, out.next - val_start);
  out.value.assign(val_sv.begin(), val_sv.end());
  return out;
}

std::map<std::string, std::string> parse_attrs(std::string_view tag_contents) {
  std::map<std::string, std::string> attrs;
  std::size_t pos = 0;
  while (pos < tag_contents.size()) {
    AttrParseResult one = parse_one_attr(tag_contents, pos);
    pos = (one.next > pos) ? one.next : (pos + 1);
    if (one.name.empty())
      continue;
    attrs[to_ascii_lower(one.name)] = one.value;
  }
  return attrs;
}

bool looks_like_unsupported_flow(const HttpResponse &response) {
  // Deterministic heuristics to detect SAML/MFA/SSO handoffs.
  if (response.status >= 300 && response.status < 400) {
    const std::string *loc = response.header_ci("location");
    if (loc) {
      const std::string ll = to_ascii_lower(*loc);
      if (ll.find("saml") != std::string::npos || ll.find("sso") != std::string::npos)
        return true;
    }
  }

  const std::string body_lower = to_ascii_lower(response.body);
  const char *needles[] = {
      "saml",       "sso",        "duo",        "two-factor",
      "multifactor", "mfa",        "okta",       "webauthn",
      "totp",       "authenticator", "azuread",   "onelogin",
  };

  for (const char *n : needles) {
    if (body_lower.find(n) != std::string::npos)
      return true;
  }
  return false;
}

std::string extract_cookie_pair(std::string_view set_cookie_value) {
  // Extract the first "name=value" segment before any attributes.
  const std::size_t semi = set_cookie_value.find(';');
  std::string_view pair = (semi == std::string_view::npos)
                              ? set_cookie_value
                              : set_cookie_value.substr(0, semi);
  pair = trim_ascii(pair);
  return std::string(pair.begin(), pair.end());
}

std::pair<std::string, std::string> split_cookie_pair(
    const std::string &cookie_pair) {
  const std::size_t eq = cookie_pair.find('=');
  if (eq == std::string::npos || eq == 0)
    return {};

  return {std::string(trim_ascii(std::string_view(cookie_pair).substr(0, eq))),
          std::string(
              trim_ascii(std::string_view(cookie_pair).substr(eq + 1)))};
}

bool starts_with_ci(std::string_view s, std::string_view prefix) {
  if (s.size() < prefix.size())
    return false;
  return iequals_ascii(s.substr(0, prefix.size()), prefix);
}

std::string json_string_value(std::string_view body, std::string_view key) {
  // Minimal JSON string extractor for bodies like:
  // {"error":"auth_failed","message":"..."}
  const std::string needle = "\"" + std::string(key) + "\"";
  const std::size_t k = body.find(needle);
  if (k == std::string_view::npos)
    return "";

  std::size_t pos = k + needle.size();
  pos = body.find(':', pos);
  if (pos == std::string_view::npos)
    return "";
  ++pos;
  while (pos < body.size() && is_ascii_space(static_cast<unsigned char>(body[pos])))
    ++pos;
  if (pos >= body.size() || body[pos] != '"')
    return "";
  ++pos;

  std::string out;
  while (pos < body.size()) {
    const char ch = body[pos++];
    if (ch == '"')
      break;
    if (ch == '\\' && pos < body.size()) {
      // Keep this conservative: for tests we only need to handle plain strings.
      out.push_back(body[pos++]);
      continue;
    }
    out.push_back(ch);
  }
  return out;
}

} // namespace

void AuthCookieJar::clear() {
  cookies_.clear();
  header_.clear();
}

void AuthCookieJar::collect_from_response(const HttpResponse &response) {
  const std::vector<std::string> *values =
      response.header_values_ci("set-cookie");
  if (!values)
    return;

  for (const std::string &raw : *values) {
    const std::string pair = extract_cookie_pair(raw);
    auto name_value = split_cookie_pair(pair);
    if (name_value.first.empty())
      continue;

    auto existing =
        std::find_if(cookies_.begin(), cookies_.end(),
                     [&](const Cookie &cookie) {
                       return cookie.name == name_value.first;
                     });
    if (existing == cookies_.end()) {
      Cookie cookie;
      cookie.name = std::move(name_value.first);
      cookie.value = std::move(name_value.second);
      cookies_.push_back(std::move(cookie));
    } else {
      existing->value = std::move(name_value.second);
    }
  }

  rebuild_header();
}

bool AuthCookieJar::empty() const { return header_.empty(); }

const std::string &AuthCookieJar::header() const { return header_; }

void AuthCookieJar::rebuild_header() {
  header_.clear();
  for (const Cookie &cookie : cookies_) {
    if (!header_.empty())
      header_ += "; ";
    header_ += cookie.name;
    header_ += "=";
    header_ += cookie.value;
  }
}

ValidationResult parse_auth_form(const std::string &html, AuthForm *out) {
  if (!out)
    return invalid("auth_form_invalid", "output pointer is null");

  std::string_view s(html);
  const std::size_t form_pos = ifind_ascii(s, "<form");
  if (form_pos == std::string_view::npos)
    return invalid("auth_form_invalid", "no <form> tag found");

  const std::size_t form_tag_end = s.find('>', form_pos);
  if (form_tag_end == std::string_view::npos)
    return invalid("auth_form_invalid", "unterminated <form> tag");

  std::string_view form_tag = s.substr(form_pos + 5, form_tag_end - (form_pos + 5));
  auto form_attrs = parse_attrs(form_tag);

  std::string action;
  {
    auto it = form_attrs.find("action");
    if (it != form_attrs.end())
      action = it->second;
  }
  if (trim_ascii(action).empty())
    return invalid("auth_form_invalid", "form action is missing");

  const std::size_t form_end = ifind_ascii(s, "</form", form_tag_end);
  const std::size_t content_end =
      (form_end == std::string_view::npos) ? s.size() : form_end;
  std::string_view content = s.substr(form_tag_end + 1, content_end - (form_tag_end + 1));

  std::string username_field;
  std::string password_field;
  std::map<std::string, std::string> hidden_fields;

  std::size_t pos = 0;
  while (pos < content.size()) {
    const std::size_t input_pos = ifind_ascii(content, "<input", pos);
    if (input_pos == std::string_view::npos)
      break;

    const std::size_t tag_end = content.find('>', input_pos);
    if (tag_end == std::string_view::npos)
      break;

    std::string_view tag =
        content.substr(input_pos + 6, tag_end - (input_pos + 6));
    auto attrs = parse_attrs(tag);

    const std::string type = to_ascii_lower(attrs["type"]);
    const std::string name = attrs["name"];
    const std::string value = attrs["value"];

    if (!name.empty()) {
      if (type == "hidden") {
        hidden_fields[name] = value;
      } else if (type == "password" || icontains_ascii(to_ascii_lower(name), "pass")) {
        if (password_field.empty())
          password_field = name;
      } else {
        const std::string lname = to_ascii_lower(name);
        const bool is_userish = (type == "text" || type == "email" || type.empty()) &&
                                (lname.find("user") != std::string::npos ||
                                 lname.find("login") != std::string::npos ||
                                 lname.find("email") != std::string::npos);
        if (username_field.empty() && is_userish)
          username_field = name;
        if (username_field.empty() && type != "hidden" && type != "password")
          username_field = name;
      }
    }

    pos = tag_end + 1;
  }

  if (username_field.empty())
    return invalid("auth_form_invalid", "username field not found");
  if (password_field.empty())
    return invalid("auth_form_invalid", "password field not found");

  AuthForm parsed;
  parsed.action_path = std::move(action);
  parsed.username_field = std::move(username_field);
  parsed.password_field = std::move(password_field);
  parsed.hidden_fields = std::move(hidden_fields);

  *out = std::move(parsed);
  return ValidationResult{};
}

AuthResult parse_auth_response(const HttpResponse &response) {
  AuthResult out;

  if (looks_like_unsupported_flow(response)) {
    out.ok = false;
    out.error_code = "unsupported_auth_flow";
    out.error_message = "unsupported authentication flow";
    return out;
  }

  if (response.status == 401 || response.status == 403) {
    const std::string error = json_string_value(response.body, "error");
    const std::string msg = json_string_value(response.body, "message");

    if (!error.empty()) {
      out.ok = false;
      out.error_code = error;
      out.error_message = msg;
      if (error != "auth_failed") {
        out.error_code = "unsupported_auth_flow";
        if (!msg.empty())
          out.error_message = msg;
        else
          out.error_message = "unsupported authentication flow";
      }
      return out;
    }

    out.ok = false;
    out.error_code = "auth_failed";
    out.error_message = "authentication failed";
    return out;
  }

  if (response.status < 200 || response.status >= 300) {
    out.ok = false;
    out.error_code = "protocol_error";
    out.error_message = "unexpected HTTP status in auth response";
    return out;
  }

  const auto *cookie_headers = response.header_values_ci("set-cookie");
  if (!cookie_headers || cookie_headers->empty()) {
    out.ok = false;
    out.error_code = "protocol_error";
    out.error_message = "missing Set-Cookie in auth response";
    return out;
  }

  std::string cookie_pair;
  for (const std::string &h : *cookie_headers) {
    const std::string pair = extract_cookie_pair(h);
    if (starts_with_ci(pair, "webvpn=")) {
      cookie_pair = pair;
      break;
    }
  }

  if (cookie_pair.empty()) {
    out.ok = false;
    out.error_code = "protocol_error";
    out.error_message = "missing webvpn cookie";
    return out;
  }

  out.ok = true;
  out.cookie = std::move(cookie_pair);
  return out;
}

} // namespace protocol
} // namespace vpn_engine
} // namespace exv
