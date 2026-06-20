#include "vpn_engine/protocol/native_auth_session_json.hpp"

#include <chrono>
#include <cstdint>
#include <iostream>
#include <limits>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

exv::vpn_engine::protocol::NativeAuthSession sample_session() {
  exv::vpn_engine::protocol::NativeAuthSession session;
  session.server.scheme = "https";
  session.server.host = "vpn.example.invalid";
  session.server.port = 8443;
  session.server.base_path = "/vpn";
  session.username = "student@example.invalid";
  session.useragent = "EXV native-auth json test";
  session.cookie_header = "webvpn=secret-cookie-value";
  session.selected_group = "student";
  session.auth_method = "password";
  session.created_at =
      std::chrono::system_clock::time_point{std::chrono::milliseconds{
          1712345678123LL}};
  session.diagnostics["auth_method"] = "password";
  session.diagnostics["cookie_present"] = "true";
  return session;
}

bool roundtrip_preserves_auth_session_fields() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  const auto original = sample_session();
  const auto payload = to_json(original);

  exv::vpn_engine::protocol::NativeAuthSession parsed;
  const auto result = from_json(payload, &parsed);

  ok = expect(result.ok, "roundtrip from_json should accept to_json payload") &&
       ok;
  ok = expect(parsed.server.scheme == original.server.scheme,
              "roundtrip should preserve server scheme") &&
       ok;
  ok = expect(parsed.server.host == original.server.host,
              "roundtrip should preserve server host") &&
       ok;
  ok = expect(parsed.server.port == original.server.port,
              "roundtrip should preserve server port") &&
       ok;
  ok = expect(parsed.server.base_path == original.server.base_path,
              "roundtrip should preserve server base_path") &&
       ok;
  ok = expect(parsed.username == original.username,
              "roundtrip should preserve username") &&
       ok;
  ok = expect(parsed.useragent == original.useragent,
              "roundtrip should preserve useragent") &&
       ok;
  ok = expect(parsed.cookie_header == original.cookie_header,
              "roundtrip should preserve cookie header") &&
       ok;
  ok = expect(parsed.selected_group == original.selected_group,
              "roundtrip should preserve selected group") &&
       ok;
  ok = expect(parsed.auth_method == original.auth_method,
              "roundtrip should preserve auth method") &&
       ok;
  ok = expect(parsed.created_at == original.created_at,
              "roundtrip should preserve created_at") &&
       ok;
  ok = expect(parsed.diagnostics == original.diagnostics,
              "roundtrip should preserve diagnostics") &&
       ok;
  return ok;
}

bool serialized_payload_does_not_contain_password_field() {
  using exv::vpn_engine::protocol::to_json;

  const auto payload = to_json(sample_session());
  return expect(!payload.contains("password"),
                "serialized payload must not contain password");
}

bool redacted_summary_excludes_cookie_value() {
  using exv::vpn_engine::protocol::summarize_native_auth_session;

  bool ok = true;
  const auto session = sample_session();
  const auto summary = summarize_native_auth_session(session);
  const std::string dumped = summary.dump();

  ok = expect(dumped.find(session.cookie_header) == std::string::npos,
              "redacted summary must not contain cookie value") &&
       ok;
  ok = expect(summary.value("cookie_present", false),
              "redacted summary should expose cookie_present") &&
       ok;
  ok = expect(summary.value("host", std::string()) == session.server.host,
              "redacted summary should include host") &&
       ok;
  ok = expect(summary.value("username_present", false),
              "redacted summary should expose username presence") &&
       ok;
  ok = expect(summary.value("useragent_present", false),
              "redacted summary should expose useragent presence") &&
       ok;
  ok = expect(summary.value("selected_group_present", false),
              "redacted summary should expose selected_group presence") &&
       ok;
  ok = expect(summary.value("auth_method", std::string()) ==
                  session.auth_method,
              "redacted summary should include auth_method") &&
       ok;
  return ok;
}

bool summary_redacts_secret_like_free_form_fields() {
  using exv::vpn_engine::protocol::summarize_native_auth_session;

  bool ok = true;
  auto session = sample_session();
  session.username = "alice token=user-token-123";
  session.useragent = "EXV password=ua-passw0rd csrf=ua-csrf-token";
  session.selected_group = "students; webvpn=group-cookie-value";
  session.diagnostics["http_status"] = "200";
  session.diagnostics["content_type"] = "text/html";
  session.diagnostics["selected_group_present"] = "true";
  session.diagnostics["auth_method"] = "password";
  session.diagnostics["body_bytes"] = "1234";
  session.diagnostics["csrf"] = "diagnostic-csrf-token";

  const auto summary = summarize_native_auth_session(session);
  const std::string dumped = summary.dump();

  for (const std::string needle :
       {"user-token-123", "ua-passw0rd", "ua-csrf-token",
        "group-cookie-value", "diagnostic-csrf-token", "webvpn="}) {
    ok = expect(dumped.find(needle) == std::string::npos,
                "summary must not contain secret-like free-form values") &&
         ok;
  }
  ok = expect(summary.value("username_present", false),
              "summary should expose username presence") &&
       ok;
  ok = expect(summary.value("useragent_present", false),
              "summary should expose useragent presence") &&
       ok;
  ok = expect(summary.value("selected_group_present", false),
              "summary should expose selected_group presence") &&
       ok;
  ok = expect(summary.value("username_length", 0) ==
                  static_cast<int>(session.username.size()),
              "summary should expose username length") &&
       ok;
  ok = expect(summary.value("useragent_length", 0) ==
                  static_cast<int>(session.useragent.size()),
              "summary should expose useragent length") &&
       ok;
  ok = expect(summary.value("selected_group_length", 0) ==
                  static_cast<int>(session.selected_group.size()),
              "summary should expose selected_group length") &&
       ok;
  ok = expect(summary.contains("diagnostics") &&
                  summary.at("diagnostics").value("http_status",
                                                  std::string()) == "200",
              "summary should preserve safe diagnostics") &&
       ok;
  ok = expect(summary.contains("diagnostics") &&
                  summary.at("diagnostics")
                          .value("selected_group_present", std::string()) ==
                      "true",
              "summary should preserve selected_group diagnostic presence") &&
       ok;
  return ok;
}

bool missing_or_empty_cookie_is_rejected_without_leaking_cookie() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  auto payload = to_json(sample_session());
  payload.erase("cookie_header");

  exv::vpn_engine::protocol::NativeAuthSession parsed;
  auto result = from_json(payload, &parsed);
  ok = expect(!result.ok, "missing cookie_header should be rejected") && ok;
  ok = expect(result.code == "auth_session_cookie_missing",
              "missing cookie_header should use deterministic error code") &&
       ok;

  payload["cookie_header"] = "";
  result = from_json(payload, &parsed);
  ok = expect(!result.ok, "empty cookie_header should be rejected") && ok;
  ok = expect(result.message.find("secret-cookie-value") == std::string::npos,
              "cookie validation error must not contain cookie value") &&
       ok;
  return ok;
}

bool cookie_header_control_chars_are_rejected_without_leaking_value() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  auto payload = to_json(sample_session());
  const std::string poisoned_cookie =
      "webvpn=secret-cookie-value\r\nInjected: yes";
  payload["cookie_header"] = poisoned_cookie;

  exv::vpn_engine::protocol::NativeAuthSession parsed;
  const auto result = from_json(payload, &parsed);
  ok = expect(!result.ok,
              "cookie_header with control chars should be rejected") &&
       ok;
  ok = expect(result.code == "auth_session_cookie_invalid",
              "bad cookie_header should use deterministic error code") &&
       ok;
  ok = expect(result.message.find("secret-cookie-value") == std::string::npos,
              "cookie_header control-char error must not contain cookie") &&
       ok;
  ok = expect(result.message.find("Injected") == std::string::npos,
              "cookie_header control-char error must not contain header text") &&
       ok;
  return ok;
}

bool useragent_control_chars_are_rejected_without_leaking_value() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  std::vector<std::string> poisoned_useragents = {
      "EXV\r\nInjected: ua",
      std::string("EXV ") + static_cast<char>(0x7f) + " native",
  };

  for (const auto &poisoned_useragent : poisoned_useragents) {
    auto payload = to_json(sample_session());
    payload["useragent"] = poisoned_useragent;

    exv::vpn_engine::protocol::NativeAuthSession parsed;
    const auto result = from_json(payload, &parsed);
    ok = expect(!result.ok,
                "useragent with control chars should be rejected") &&
         ok;
    ok = expect(result.code == "auth_session_useragent_invalid",
                "bad useragent should use deterministic error code") &&
         ok;
    ok = expect(result.message.find("EXV") == std::string::npos,
                "useragent control-char error must not contain useragent") &&
         ok;
    ok = expect(result.message.find("Injected") == std::string::npos,
                "useragent control-char error must not contain header text") &&
         ok;
  }
  return ok;
}

bool unsupported_schema_version_is_rejected() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  auto payload = to_json(sample_session());
  payload["schema_version"] = 2;

  exv::vpn_engine::protocol::NativeAuthSession parsed;
  const auto result = from_json(payload, &parsed);
  return expect(!result.ok, "unsupported schema_version should be rejected") &&
         expect(result.code == "auth_session_schema_unsupported",
                "unsupported schema_version should use deterministic code");
}

bool schema_version_must_be_exact_integer_one() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  std::vector<nlohmann::json> invalid_versions = {
      nlohmann::json(static_cast<std::uint64_t>(4294967297ULL)),
      nlohmann::json(std::numeric_limits<std::uint64_t>::max()),
      nlohmann::json(-1),
      nlohmann::json(1.0),
      nlohmann::json("1"),
  };

  for (const auto &version : invalid_versions) {
    auto payload = to_json(sample_session());
    payload["schema_version"] = version;

    exv::vpn_engine::protocol::NativeAuthSession parsed;
    const auto result = from_json(payload, &parsed);
    ok = expect(!result.ok,
                "schema_version other than exact integer 1 should reject") &&
         ok;
    ok = expect(result.code == "auth_session_schema_unsupported",
                "bad schema_version should use deterministic code") &&
         ok;
    ok = expect(result.message.find("secret-cookie-value") ==
                    std::string::npos,
                "schema_version error must not leak cookie") &&
         ok;
  }
  return ok;
}

bool malformed_server_is_rejected() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  auto payload = to_json(sample_session());
  payload["server"]["host"] = "";

  exv::vpn_engine::protocol::NativeAuthSession parsed;
  const auto result = from_json(payload, &parsed);
  ok = expect(!result.ok, "empty server host should be rejected") && ok;
  ok = expect(result.code == "auth_session_server_invalid",
              "malformed server should use deterministic code") &&
       ok;
  ok = expect(result.message.find("secret-cookie-value") == std::string::npos,
              "server validation error must not contain cookie value") &&
       ok;
  return ok;
}

bool secret_like_diagnostics_are_not_serialized_or_summarized() {
  using exv::vpn_engine::protocol::summarize_native_auth_session;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  auto session = sample_session();
  session.diagnostics["cookie"] = "webvpn=diagnostic-cookie";
  session.diagnostics["password"] = "diagnostic-password";
  session.diagnostics["token"] = "diagnostic-token";
  session.diagnostics["secret"] = "diagnostic-secret";
  session.diagnostics["saml"] = "diagnostic-saml";
  session.diagnostics["csrf"] = "diagnostic-csrf";
  session.diagnostics["challenge"] = "diagnostic-challenge-response";
  session.diagnostics["http_status"] = "200";

  const auto payload = to_json(session);
  const std::string serialized = payload.dump();
  const std::string summary = summarize_native_auth_session(session).dump();

  ok = expect(payload.at("diagnostics").contains("http_status"),
              "safe diagnostics should be serialized") &&
       ok;
  for (const std::string needle :
       {"diagnostic-cookie", "diagnostic-password", "diagnostic-token",
        "diagnostic-secret", "diagnostic-saml", "diagnostic-csrf",
        "diagnostic-challenge-response"}) {
    ok = expect(serialized.find(needle) == std::string::npos,
                "serialized JSON must not contain secret-like diagnostics") &&
         ok;
    ok = expect(summary.find(needle) == std::string::npos,
                "redacted summary must not contain secret-like diagnostics") &&
         ok;
  }
  return ok;
}

bool public_saml_diagnostic_roundtrips_without_secrets() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::summarize_native_auth_session;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  auto session = sample_session();
  session.diagnostics["saml_required"] = "true";
  session.diagnostics["http_status"] = "200";
  session.diagnostics["content_type"] = "text/html";
  session.diagnostics["body_bytes"] = "512";

  const auto payload = to_json(session);
  const auto summary = summarize_native_auth_session(session);

  ok = expect(payload.at("diagnostics").value("saml_required",
                                              std::string()) == "true",
              "safe SAML diagnostic should be serialized") &&
       ok;
  ok = expect(summary.at("diagnostics").value("saml_required",
                                              std::string()) == "true",
              "safe SAML diagnostic should be summarized") &&
       ok;

  exv::vpn_engine::protocol::NativeAuthSession parsed;
  const auto result = from_json(payload, &parsed);
  ok = expect(result.ok, "safe SAML diagnostic should parse") && ok;
  ok = expect(parsed.diagnostics.at("saml_required") == "true",
              "safe SAML diagnostic should roundtrip") &&
       ok;

  auto unsafe_payload = payload;
  unsafe_payload["diagnostics"]["saml_url"] =
      "https://idp.example.invalid/sso?SAMLRequest=SECRET";
  const auto unsafe = from_json(unsafe_payload, &parsed);
  ok = expect(!unsafe.ok,
              "SAML URL diagnostic should be rejected as unsafe") &&
       ok;
  ok = expect(unsafe.message.find("idp.example.invalid") == std::string::npos,
              "unsafe SAML diagnostic error must not leak URL") &&
       ok;
  return ok;
}

bool unsafe_or_malformed_diagnostics_are_rejected_on_parse() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  std::vector<nlohmann::json> bad_diagnostics = {
      nlohmann::json("not-an-object"),
      nlohmann::json{{"http_status", 200}},
      nlohmann::json{{"cookie", "webvpn=diagnostic-cookie"}},
      nlohmann::json{{"auth_method", "contains token value"}},
      nlohmann::json{{"csrf", "diagnostic-csrf"}},
      nlohmann::json{{"challenge", "diagnostic-challenge-response"}},
  };

  for (const auto &diagnostics : bad_diagnostics) {
    auto payload = to_json(sample_session());
    payload["diagnostics"] = diagnostics;

    exv::vpn_engine::protocol::NativeAuthSession parsed;
    const auto result = from_json(payload, &parsed);
    ok = expect(!result.ok,
                "unsafe or malformed diagnostics should be rejected") &&
         ok;
    ok = expect(result.code == "auth_session_diagnostics_invalid",
                "bad diagnostics should use deterministic code") &&
         ok;
    ok = expect(result.message.find("diagnostic-cookie") == std::string::npos,
                "diagnostics error must not leak secret diagnostic value") &&
         ok;
  }
  return ok;
}

bool malformed_required_fields_are_rejected() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;

  struct Case {
    const char *field;
    nlohmann::json value;
    const char *expected_code;
  };

  std::vector<Case> wrong_type_cases = {
      {"username", 42, "auth_session_username_missing"},
      {"useragent", false, "auth_session_useragent_missing"},
      {"cookie_header", nlohmann::json::array(), "auth_session_cookie_missing"},
      {"auth_method", nullptr, "auth_session_auth_method_missing"},
      {"created_at_unix_ms", "1712345678123",
       "auth_session_created_at_missing"},
      {"selected_group", 42, "auth_session_field_invalid"},
  };

  for (const auto &test_case : wrong_type_cases) {
    auto payload = to_json(sample_session());
    payload[test_case.field] = test_case.value;

    exv::vpn_engine::protocol::NativeAuthSession parsed;
    const auto result = from_json(payload, &parsed);
    ok = expect(!result.ok, "wrong required field type should reject") && ok;
    ok = expect(result.code == test_case.expected_code,
                "wrong required field type should use deterministic code") &&
         ok;
  }

  std::vector<Case> missing_cases = {
      {"username", nullptr, "auth_session_username_missing"},
      {"useragent", nullptr, "auth_session_useragent_missing"},
      {"cookie_header", nullptr, "auth_session_cookie_missing"},
      {"auth_method", nullptr, "auth_session_auth_method_missing"},
      {"created_at_unix_ms", nullptr, "auth_session_created_at_missing"},
  };

  for (const auto &test_case : missing_cases) {
    auto payload = to_json(sample_session());
    payload.erase(test_case.field);

    exv::vpn_engine::protocol::NativeAuthSession parsed;
    const auto result = from_json(payload, &parsed);
    ok = expect(!result.ok, "missing required field should reject") && ok;
    ok = expect(result.code == test_case.expected_code,
                "missing required field should use deterministic code") &&
         ok;
  }
  return ok;
}

bool server_port_boundaries_are_rejected() {
  using exv::vpn_engine::protocol::from_json;
  using exv::vpn_engine::protocol::to_json;

  bool ok = true;
  for (const auto &port : {nlohmann::json(0), nlohmann::json(-1),
                           nlohmann::json(65536), nlohmann::json("443")}) {
    auto payload = to_json(sample_session());
    payload["server"]["port"] = port;

    exv::vpn_engine::protocol::NativeAuthSession parsed;
    const auto result = from_json(payload, &parsed);
    ok = expect(!result.ok, "invalid server port should reject") && ok;
    ok = expect(result.code == "auth_session_server_invalid",
                "invalid server port should use deterministic code") &&
         ok;
  }
  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = roundtrip_preserves_auth_session_fields() && ok;
  ok = serialized_payload_does_not_contain_password_field() && ok;
  ok = redacted_summary_excludes_cookie_value() && ok;
  ok = missing_or_empty_cookie_is_rejected_without_leaking_cookie() && ok;
  ok = cookie_header_control_chars_are_rejected_without_leaking_value() && ok;
  ok = useragent_control_chars_are_rejected_without_leaking_value() && ok;
  ok = unsupported_schema_version_is_rejected() && ok;
  ok = schema_version_must_be_exact_integer_one() && ok;
  ok = malformed_server_is_rejected() && ok;
  ok = summary_redacts_secret_like_free_form_fields() && ok;
  ok = secret_like_diagnostics_are_not_serialized_or_summarized() && ok;
  ok = public_saml_diagnostic_roundtrips_without_secrets() && ok;
  ok = unsafe_or_malformed_diagnostics_are_rejected_on_parse() && ok;
  ok = malformed_required_fields_are_rejected() && ok;
  ok = server_port_boundaries_are_rejected() && ok;

  return ok ? 0 : 1;
}
