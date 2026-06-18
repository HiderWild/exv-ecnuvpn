#include "vpn_engine/protocol/native_auth_session_json.hpp"

#include <array>
#include <cctype>
#include <chrono>
#include <cstdint>
#include <limits>
#include <map>
#include <string>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

namespace {

constexpr int kNativeAuthSessionSchemaVersion = 1;

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

std::string to_ascii_lower(const std::string &input) {
  std::string out;
  out.reserve(input.size());
  for (unsigned char ch : input)
    out.push_back(static_cast<char>(std::tolower(ch)));
  return out;
}

bool json_integer_equals(const nlohmann::json &value, std::int64_t expected) {
  if (value.type() == nlohmann::json::value_t::number_integer)
    return value.get<std::int64_t>() == expected;
  if (value.type() == nlohmann::json::value_t::number_unsigned)
    return expected >= 0 &&
           value.get<std::uint64_t>() == static_cast<std::uint64_t>(expected);
  return false;
}

bool json_integer_in_range(const nlohmann::json &value, std::int64_t min,
                           std::int64_t max, std::int64_t *out) {
  if (!out)
    return false;

  if (value.type() == nlohmann::json::value_t::number_integer) {
    const auto parsed = value.get<std::int64_t>();
    if (parsed < min || parsed > max)
      return false;
    *out = parsed;
    return true;
  }

  if (value.type() == nlohmann::json::value_t::number_unsigned) {
    if (max < 0)
      return false;
    const auto parsed = value.get<std::uint64_t>();
    if (parsed > static_cast<std::uint64_t>(max))
      return false;
    if (min > 0 && parsed < static_cast<std::uint64_t>(min))
      return false;
    *out = static_cast<std::int64_t>(parsed);
    return true;
  }

  return false;
}

std::int64_t unix_ms_from_time_point(
    const std::chrono::system_clock::time_point &time_point) {
  return std::chrono::duration_cast<std::chrono::milliseconds>(
             time_point.time_since_epoch())
      .count();
}

std::chrono::system_clock::time_point
time_point_from_unix_ms(std::int64_t unix_ms) {
  return std::chrono::system_clock::time_point{
      std::chrono::milliseconds{unix_ms}};
}

bool get_required_string(const nlohmann::json &object, const char *field,
                         std::string *out) {
  if (!out || !object.contains(field) || !object.at(field).is_string())
    return false;

  *out = object.at(field).get<std::string>();
  return !out->empty();
}

bool get_optional_string(const nlohmann::json &object, const char *field,
                         std::string *out) {
  if (!out || !object.contains(field)) {
    if (out)
      out->clear();
    return true;
  }

  if (!object.at(field).is_string())
    return false;

  *out = object.at(field).get<std::string>();
  return true;
}

bool contains_header_control_char(const std::string &value) {
  for (unsigned char ch : value) {
    if (ch < 0x20 || ch == 0x7f)
      return true;
  }
  return false;
}

bool parse_server(const nlohmann::json &payload, ParsedVpnUrl *server) {
  if (!server || !payload.contains("server") || !payload.at("server").is_object())
    return false;

  const auto &server_json = payload.at("server");
  ParsedVpnUrl parsed;
  if (!get_required_string(server_json, "scheme", &parsed.scheme))
    return false;
  if (parsed.scheme != "https")
    return false;
  if (!get_required_string(server_json, "host", &parsed.host))
    return false;
  if (!server_json.contains("port"))
    return false;

  std::int64_t port = 0;
  if (!json_integer_in_range(server_json.at("port"), 1, 65535, &port))
    return false;
  parsed.port = static_cast<int>(port);

  if (!get_required_string(server_json, "base_path", &parsed.base_path))
    return false;
  if (parsed.base_path.front() != '/')
    return false;

  *server = std::move(parsed);
  return true;
}

bool diagnostic_key_is_allowed(const std::string &key) {
  static constexpr std::array<const char *, 7> kAllowedKeys = {
      "cookie_present", "auth_method", "http_status",
      "content_type", "body_bytes", "selected_group_present",
      "saml_required"};

  for (const char *allowed : kAllowedKeys) {
    if (key == allowed)
      return true;
  }
  return false;
}

bool diagnostic_value_is_secret_like(const std::string &key,
                                     const std::string &value) {
  const std::string lower = to_ascii_lower(value);
  if (key == "auth_method" && lower == "password")
    return false;

  static constexpr std::array<const char *, 8> kSecretMarkers = {
      "cookie", "password", "token", "secret", "saml", "csrf",
      "challenge", "webvpn="};

  for (const char *marker : kSecretMarkers) {
    if (lower.find(marker) != std::string::npos)
      return true;
  }
  return false;
}

bool diagnostic_entry_is_safe(const std::string &key,
                              const std::string &value) {
  return diagnostic_key_is_allowed(key) &&
         !diagnostic_value_is_secret_like(key, value);
}

nlohmann::json safe_diagnostics_json(
    const std::map<std::string, std::string> &diagnostics) {
  nlohmann::json safe = nlohmann::json::object();
  for (const auto &entry : diagnostics) {
    if (diagnostic_entry_is_safe(entry.first, entry.second))
      safe[entry.first] = entry.second;
  }
  return safe;
}

bool parse_diagnostics(const nlohmann::json &payload,
                       std::map<std::string, std::string> *diagnostics) {
  if (!diagnostics)
    return false;

  diagnostics->clear();
  if (!payload.contains("diagnostics"))
    return true;
  if (!payload.at("diagnostics").is_object())
    return false;

  for (const auto &entry : payload.at("diagnostics").items()) {
    if (!entry.value().is_string())
      return false;
    const std::string value = entry.value().get<std::string>();
    if (!diagnostic_entry_is_safe(entry.key(), value))
      return false;
    (*diagnostics)[entry.key()] = value;
  }
  return true;
}

} // namespace

nlohmann::json to_json(const NativeAuthSession &session) {
  return nlohmann::json{
      {"schema_version", kNativeAuthSessionSchemaVersion},
      {"server",
       nlohmann::json{{"scheme", session.server.scheme},
                      {"host", session.server.host},
                      {"port", session.server.port},
                      {"base_path", session.server.base_path}}},
      {"username", session.username},
      {"useragent", session.useragent},
      {"cookie_header", session.cookie_header},
      {"selected_group", session.selected_group},
      {"auth_method", session.auth_method},
      {"created_at_unix_ms", unix_ms_from_time_point(session.created_at)},
      {"diagnostics", safe_diagnostics_json(session.diagnostics)}};
}

ValidationResult from_json(const nlohmann::json &payload,
                           NativeAuthSession *session) {
  if (!session)
    return invalid("auth_session_output_missing",
                   "native auth session output must not be null");
  if (!payload.is_object())
    return invalid("auth_session_invalid",
                   "native auth session payload must be a JSON object");

  if (!payload.contains("schema_version") ||
      !json_integer_equals(payload.at("schema_version"),
                           kNativeAuthSessionSchemaVersion)) {
    return invalid("auth_session_schema_unsupported",
                   "unsupported native auth session schema_version");
  }

  NativeAuthSession parsed;
  if (!parse_server(payload, &parsed.server))
    return invalid("auth_session_server_invalid",
                   "native auth session server is invalid");

  if (!get_required_string(payload, "username", &parsed.username))
    return invalid("auth_session_username_missing",
                   "native auth session username is required");
  if (!get_required_string(payload, "useragent", &parsed.useragent))
    return invalid("auth_session_useragent_missing",
                   "native auth session useragent is required");
  if (contains_header_control_char(parsed.useragent))
    return invalid("auth_session_useragent_invalid",
                   "native auth session useragent contains invalid characters");
  if (!get_required_string(payload, "cookie_header", &parsed.cookie_header))
    return invalid("auth_session_cookie_missing",
                   "native auth session cookie_header is required");
  if (contains_header_control_char(parsed.cookie_header))
    return invalid(
        "auth_session_cookie_invalid",
        "native auth session cookie_header contains invalid characters");

  if (!get_optional_string(payload, "selected_group", &parsed.selected_group))
    return invalid("auth_session_field_invalid",
                   "native auth session selected_group must be a string");
  if (!get_required_string(payload, "auth_method", &parsed.auth_method))
    return invalid("auth_session_auth_method_missing",
                   "native auth session auth_method is required");

  if (!payload.contains("created_at_unix_ms")) {
    return invalid("auth_session_created_at_missing",
                   "native auth session created_at_unix_ms is required");
  }
  std::int64_t created_at_unix_ms = 0;
  if (!json_integer_in_range(payload.at("created_at_unix_ms"),
                             std::numeric_limits<std::int64_t>::min(),
                             std::numeric_limits<std::int64_t>::max(),
                             &created_at_unix_ms)) {
    return invalid("auth_session_created_at_missing",
                   "native auth session created_at_unix_ms is required");
  }
  parsed.created_at = time_point_from_unix_ms(created_at_unix_ms);

  if (!parse_diagnostics(payload, &parsed.diagnostics))
    return invalid("auth_session_diagnostics_invalid",
                   "native auth session diagnostics must be a string object");

  *session = std::move(parsed);
  return ValidationResult{};
}

nlohmann::json summarize_native_auth_session(const NativeAuthSession &session) {
  return nlohmann::json{
      {"schema_version", kNativeAuthSessionSchemaVersion},
      {"host", session.server.host},
      {"username_present", !session.username.empty()},
      {"username_length", session.username.size()},
      {"useragent_present", !session.useragent.empty()},
      {"useragent_length", session.useragent.size()},
      {"cookie_present", !session.cookie_header.empty()},
      {"selected_group_present", !session.selected_group.empty()},
      {"selected_group_length", session.selected_group.size()},
      {"auth_method", session.auth_method},
      {"created_at_unix_ms", unix_ms_from_time_point(session.created_at)},
      {"diagnostics", safe_diagnostics_json(session.diagnostics)}};
}

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
