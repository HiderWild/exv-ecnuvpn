#include "vpn_engine/protocol/http.hpp"

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

bool starts_with(std::string_view s, std::string_view prefix) {
  return s.size() >= prefix.size() && s.substr(0, prefix.size()) == prefix;
}

} // namespace

const std::string *HttpResponse::header_ci(const std::string &name) const {
  const std::string key = to_ascii_lower(name);
  auto it = headers.find(key);
  if (it == headers.end())
    return nullptr;
  return &it->second;
}

const std::vector<std::string> *HttpResponse::header_values_ci(
    const std::string &name) const {
  const std::string key = to_ascii_lower(name);
  auto it = header_values.find(key);
  if (it == header_values.end())
    return nullptr;
  return &it->second;
}

ValidationResult parse_http_response(const std::string &raw, HttpResponse *out) {
  if (!out)
    return invalid("http_invalid", "output pointer is null");

  std::string_view s(raw);
  std::size_t header_end = s.find("\r\n\r\n");
  std::size_t delim_len = 4;
  if (header_end == std::string_view::npos) {
    header_end = s.find("\n\n");
    delim_len = 2;
  }
  if (header_end == std::string_view::npos)
    return invalid("http_invalid", "missing header terminator");

  std::string_view header_block = s.substr(0, header_end);
  std::string_view body = s.substr(header_end + delim_len);

  if (header_block.empty())
    return invalid("http_invalid", "missing status line");

  bool first_line = true;
  int status = 0;
  std::map<std::string, std::string> headers;
  std::map<std::string, std::vector<std::string>> header_values;

  while (!header_block.empty()) {
    const std::size_t nl = header_block.find('\n');
    std::string_view line = (nl == std::string_view::npos)
                                ? header_block
                                : header_block.substr(0, nl);
    header_block = (nl == std::string_view::npos)
                       ? std::string_view()
                       : header_block.substr(nl + 1);

    if (!line.empty() && line.back() == '\r')
      line.remove_suffix(1);

    if (first_line) {
      first_line = false;
      line = trim_ascii(line);
      if (!starts_with(line, "HTTP/"))
        return invalid("http_invalid", "invalid status line");

      const std::size_t sp1 = line.find(' ');
      if (sp1 == std::string_view::npos)
        return invalid("http_invalid", "invalid status line");

      std::string_view rest = trim_ascii(line.substr(sp1 + 1));
      int code = 0;
      int digits = 0;
      for (unsigned char ch : rest) {
        if (ch < '0' || ch > '9')
          break;
        code = code * 10 + (ch - '0');
        ++digits;
        if (digits > 3)
          break;
      }
      if (digits != 3)
        return invalid("http_invalid", "invalid status code");
      if (code < 100 || code > 599)
        return invalid("http_invalid", "status code out of range");
      status = code;
      continue;
    }

    if (line.empty())
      continue;

    const std::size_t colon = line.find(':');
    if (colon == std::string_view::npos)
      return invalid("http_invalid", "malformed header line");

    std::string_view name = trim_ascii(line.substr(0, colon));
    std::string_view value = trim_ascii(line.substr(colon + 1));
    if (name.empty())
      return invalid("http_invalid", "empty header name");

    const std::string key = to_ascii_lower(name);
    header_values[key].push_back(std::string(value));

    auto it = headers.find(key);
    if (it == headers.end()) {
      headers.emplace(key, std::string(value));
    } else if (!value.empty()) {
      if (!it->second.empty())
        it->second.append(", ");
      it->second.append(value.begin(), value.end());
    }
  }

  if (first_line)
    return invalid("http_invalid", "missing status line");

  HttpResponse parsed;
  parsed.status = status;
  parsed.headers = std::move(headers);
  parsed.header_values = std::move(header_values);
  parsed.body.assign(body.begin(), body.end());

  *out = std::move(parsed);
  return ValidationResult{};
}

} // namespace protocol
} // namespace vpn_engine
} // namespace exv
