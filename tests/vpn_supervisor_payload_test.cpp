#include "platform/common/vpn_supervisor_process.hpp"
#include "vpn_engine/protocol/native_auth_session_json.hpp"

#include <chrono>
#include <cstdint>
#include <iostream>
#include <string>

namespace {

static const char *MOCK_PASSWORD = "test-mock-password-placeholder";

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

ecnuvpn::Config sample_config() {
  ecnuvpn::Config config;
  config.server = "https://vpn.example.invalid";
  config.username = "student@example.invalid";
  config.password = MOCK_PASSWORD;
  config.mtu = 1280;
  config.useragent = "ECNU-VPN supervisor payload test";
  config.disable_dtls = true;
  config.remember_password = false;
  config.routes = {"10.0.0.0/8", "192.0.2.1"};
  config.extra_args = {"--test-arg"};
  config.log_file = "supervisor-test.log";
  config.webui_port = 18081;
  config.webui_bind = "127.0.0.1";
  config.webui_enabled = false;
  config.vpn_engine = "native";
  config.openconnect_runtime = "bundled";
  config.windows_tunnel_driver = "wintun";
  config.windows_tap_interface = "ECNU VPN";
  config.auto_reconnect = false;
  config.minimal_mode = true;
  config.service_install_prompt_seen = true;
  config.minimal_install_service_before_connect = false;
  return config;
}

ecnuvpn::vpn_engine::protocol::NativeAuthSession sample_auth_session() {
  ecnuvpn::vpn_engine::protocol::NativeAuthSession session;
  session.server.scheme = "https";
  session.server.host = "vpn.example.invalid";
  session.server.port = 8443;
  session.server.base_path = "/vpn";
  session.username = "student@example.invalid";
  session.useragent = "ECNU-VPN supervisor auth-session test";
  session.cookie_header = "webvpn=super-cookie-secret";
  session.selected_group = "student";
  session.auth_method = "password";
  session.created_at =
      std::chrono::system_clock::time_point{std::chrono::milliseconds{
          1712345678123LL}};
  session.diagnostics["auth_method"] = "password";
  session.diagnostics["cookie_present"] = "true";
  return session;
}

bool auth_sessions_equal(
    const ecnuvpn::vpn_engine::protocol::NativeAuthSession &left,
    const ecnuvpn::vpn_engine::protocol::NativeAuthSession &right) {
  return left.server.scheme == right.server.scheme &&
         left.server.host == right.server.host &&
         left.server.port == right.server.port &&
         left.server.base_path == right.server.base_path &&
         left.username == right.username && left.useragent == right.useragent &&
         left.cookie_header == right.cookie_header &&
         left.selected_group == right.selected_group &&
         left.auth_method == right.auth_method &&
         left.created_at == right.created_at &&
         left.diagnostics == right.diagnostics;
}

std::string result_text(const ecnuvpn::vpn_engine::ValidationResult &result) {
  return result.code + " " + result.message;
}

bool expect_code(const ecnuvpn::vpn_engine::ValidationResult &result,
                 const char *code, const char *message) {
  return expect(result.code == code, message);
}

bool expect_no_leak(const ecnuvpn::vpn_engine::ValidationResult &result,
                    const std::string &secret, const char *message) {
  return expect(result_text(result).find(secret) == std::string::npos, message);
}

bool legacy_payload_missing_mode_decodes_password_mode_and_preserves_password() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::decode_vpn_supervisor_payload;

  const auto config = sample_config();
  nlohmann::json payload{{"config", config},
                         {"password", MOCK_PASSWORD},
                         {"retry_limit", 7},
                         {"home", "C:/Users/supervisor"},
                         {"config_dir", "C:/Users/supervisor/.ecnu-vpn"}};

  SupervisorStartPayload decoded;
  const auto result = decode_vpn_supervisor_payload(payload, &decoded);

  bool ok = true;
  ok = expect(result.ok, "legacy payload should decode") && ok;
  ok = expect(decoded.native_start_mode == SupervisorStartMode::password,
              "missing native_start_mode should default to password") &&
       ok;
  ok = expect(decoded.password == MOCK_PASSWORD,
              "legacy top-level password should be preserved") &&
       ok;
  ok = expect(decoded.config.password == config.password,
              "legacy config password should be preserved") &&
       ok;
  ok = expect(!decoded.auth_session.has_value(),
              "legacy password payload should not synthesize auth_session") &&
       ok;
  ok = expect(decoded.retry_limit == 7, "retry_limit should decode") && ok;
  ok = expect(decoded.home == "C:/Users/supervisor", "home should decode") &&
       ok;
  ok = expect(decoded.config_dir == "C:/Users/supervisor/.ecnu-vpn",
              "config_dir should decode") &&
       ok;
  return ok;
}

bool password_mode_rejects_auth_session_and_missing_or_invalid_password() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::decode_vpn_supervisor_payload;
  using ecnuvpn::platform::encode_vpn_supervisor_payload;

  bool ok = true;

  SupervisorStartPayload with_auth_session;
  with_auth_session.config = sample_config();
  with_auth_session.password = MOCK_PASSWORD;
  with_auth_session.auth_session = sample_auth_session();
  with_auth_session.native_start_mode = SupervisorStartMode::password;

  nlohmann::json encoded;
  auto result = encode_vpn_supervisor_payload(with_auth_session, &encoded);
  ok = expect(!result.ok,
              "password encode should reject auth_session presence") &&
       ok;
  ok = expect_code(result, "supervisor_auth_session_forbidden",
                   "password encode auth_session rejection code") &&
       ok;
  ok = expect_no_leak(result, MOCK_PASSWORD,
                      "password encode auth_session error must not leak password") &&
       ok;
  ok = expect_no_leak(result, "super-cookie-secret",
                      "password encode auth_session error must not leak cookie") &&
       ok;

  nlohmann::json auth_session_payload{
      {"config", sample_config()},
      {"native_start_mode", "password"},
      {"password", MOCK_PASSWORD},
      {"auth_session", ecnuvpn::vpn_engine::protocol::to_json(
                           sample_auth_session())}};
  SupervisorStartPayload decoded;
  result = decode_vpn_supervisor_payload(auth_session_payload, &decoded);
  ok = expect(!result.ok,
              "password decode should reject auth_session presence") &&
       ok;
  ok = expect_code(result, "supervisor_auth_session_forbidden",
                   "password decode auth_session rejection code") &&
       ok;
  ok = expect_no_leak(result, MOCK_PASSWORD,
                      "password decode auth_session error must not leak password") &&
       ok;
  ok = expect_no_leak(result, "super-cookie-secret",
                      "password decode auth_session error must not leak cookie") &&
       ok;

  nlohmann::json missing_password{{"config", sample_config()},
                                  {"native_start_mode", "password"}};
  result = decode_vpn_supervisor_payload(missing_password, &decoded);
  ok = expect(!result.ok, "password decode should reject missing password") &&
       ok;
  ok = expect_code(result, "supervisor_password_missing",
                   "missing password rejection code") &&
       ok;
  ok = expect_no_leak(result, MOCK_PASSWORD,
                      "missing password error must not leak config password") &&
       ok;

  nlohmann::json non_string_password{{"config", sample_config()},
                                     {"native_start_mode", "password"},
                                     {"password", 42}};
  result = decode_vpn_supervisor_payload(non_string_password, &decoded);
  ok = expect(!result.ok, "password decode should reject non-string password") &&
       ok;
  ok = expect_code(result, "supervisor_password_missing",
                   "non-string password rejection code") &&
       ok;
  ok = expect_no_leak(result, MOCK_PASSWORD,
                      "non-string password error must not leak config password") &&
       ok;

  return ok;
}

bool explicit_password_mode_roundtrip_preserves_password_without_auth_session() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::decode_vpn_supervisor_payload;
  using ecnuvpn::platform::encode_vpn_supervisor_payload;

  SupervisorStartPayload original;
  original.config = sample_config();
  original.password = MOCK_PASSWORD;
  original.retry_limit = 2;
  original.home = "/home/student";
  original.config_dir = "/home/student/.config/ecnu-vpn";
  original.native_start_mode = SupervisorStartMode::password;

  nlohmann::json encoded;
  auto result = encode_vpn_supervisor_payload(original, &encoded);

  bool ok = true;
  ok = expect(result.ok, "password payload should encode") && ok;
  ok = expect(encoded.value("native_start_mode", std::string()) == "password",
              "password payload should encode explicit mode") &&
       ok;
  ok = expect(encoded.value("password", std::string()) ==
                  MOCK_PASSWORD,
              "password payload should include top-level password") &&
       ok;
  ok = expect(!encoded.contains("auth_session"),
              "password payload should not require auth_session") &&
       ok;

  SupervisorStartPayload decoded;
  result = decode_vpn_supervisor_payload(encoded, &decoded);
  ok = expect(result.ok, "encoded password payload should decode") && ok;
  ok = expect(decoded.native_start_mode == SupervisorStartMode::password,
              "password mode should roundtrip") &&
       ok;
  ok = expect(decoded.password == original.password,
              "top-level password should roundtrip") &&
       ok;
  ok = expect(decoded.config.password == original.config.password,
              "password mode config should preserve legacy config behavior") &&
       ok;
  ok = expect(!decoded.auth_session.has_value(),
              "password mode should not produce auth_session") &&
       ok;
  return ok;
}

bool auth_session_roundtrip_omits_password_and_clears_config_password() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::decode_vpn_supervisor_payload;
  using ecnuvpn::platform::encode_vpn_supervisor_payload;

  SupervisorStartPayload original;
  original.config = sample_config();
  original.config.password = MOCK_PASSWORD;
  original.auth_session = sample_auth_session();
  original.retry_limit = -1;
  original.home = "/home/student";
  original.config_dir = "/home/student/.config/ecnu-vpn";
  original.native_start_mode = SupervisorStartMode::auth_session;

  nlohmann::json encoded;
  auto result = encode_vpn_supervisor_payload(original, &encoded);

  bool ok = true;
  ok = expect(result.ok, "auth_session payload should encode") && ok;
  ok = expect(encoded.value("native_start_mode", std::string()) ==
                  "auth_session",
              "auth_session payload should encode explicit mode") &&
       ok;
  ok = expect(!encoded.contains("password"),
              "auth_session payload must not contain top-level password") &&
       ok;
  ok = expect(!encoded.at("config").contains("password"),
              "auth_session payload config must not contain password field") &&
       ok;
  ok = expect(encoded.contains("auth_session") &&
                  encoded.at("auth_session").is_object(),
              "auth_session payload should contain auth_session object") &&
       ok;

  SupervisorStartPayload decoded;
  result = decode_vpn_supervisor_payload(encoded, &decoded);
  ok = expect(result.ok, "encoded auth_session payload should decode") && ok;
  ok = expect(decoded.native_start_mode == SupervisorStartMode::auth_session,
              "auth_session mode should roundtrip") &&
       ok;
  ok = expect(decoded.password.empty(),
              "decoded auth_session top-level password should be empty") &&
       ok;
  ok = expect(decoded.config.password.empty(),
              "decoded auth_session config.password should be empty") &&
       ok;
  ok = expect(decoded.retry_limit == -1,
              "auth_session payload should preserve steady-state retry policy") &&
       ok;
  ok = expect(decoded.auth_session.has_value(),
              "decoded auth_session payload should have auth_session") &&
       ok;
  ok = expect(decoded.auth_session.has_value() &&
                  auth_sessions_equal(*decoded.auth_session,
                                      *original.auth_session),
              "auth_session fields should roundtrip") &&
       ok;
  return ok;
}

bool auth_session_mode_with_non_empty_password_is_rejected_without_leak() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::decode_vpn_supervisor_payload;
  using ecnuvpn::platform::encode_vpn_supervisor_payload;

  bool ok = true;

  SupervisorStartPayload original;
  original.config = sample_config();
  original.password = MOCK_PASSWORD;
  original.auth_session = sample_auth_session();
  original.native_start_mode = SupervisorStartMode::auth_session;

  nlohmann::json encoded;
  auto result = encode_vpn_supervisor_payload(original, &encoded);
  std::string text = result_text(result);
  ok = expect(!result.ok,
              "auth_session encode should reject non-empty top-level password") &&
       ok;
  ok = expect_code(result, "supervisor_password_forbidden",
                   "auth_session encode non-empty password rejection code") &&
       ok;
  ok = expect(text.find(MOCK_PASSWORD) == std::string::npos,
              "password rejection error must not leak top-level password") &&
       ok;
  ok = expect(text.find("super-cookie-secret") == std::string::npos,
              "password rejection error must not leak cookie") &&
       ok;

  nlohmann::json payload{{"config", sample_config()},
                         {"native_start_mode", "auth_session"},
                         {"password", MOCK_PASSWORD},
                         {"auth_session", ecnuvpn::vpn_engine::protocol::to_json(
                                              sample_auth_session())}};
  ecnuvpn::platform::SupervisorStartPayload decoded;
  result = decode_vpn_supervisor_payload(payload, &decoded);
  text = result_text(result);
  ok = expect(!result.ok,
              "auth_session decode should reject non-empty top-level password") &&
       ok;
  ok = expect_code(result, "supervisor_password_forbidden",
                   "auth_session decode non-empty password rejection code") &&
       ok;
  ok = expect(text.find(MOCK_PASSWORD) == std::string::npos,
              "decode password rejection error must not leak password") &&
       ok;
  ok = expect(text.find("super-cookie-secret") == std::string::npos,
              "decode password rejection error must not leak cookie") &&
       ok;
  return ok;
}

bool auth_session_mode_missing_or_invalid_auth_session_is_rejected_without_leak() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::decode_vpn_supervisor_payload;
  using ecnuvpn::platform::encode_vpn_supervisor_payload;

  bool ok = true;

  SupervisorStartPayload missing;
  missing.config = sample_config();
  missing.native_start_mode = SupervisorStartMode::auth_session;
  nlohmann::json encoded;
  auto result = encode_vpn_supervisor_payload(missing, &encoded);
  ok = expect(!result.ok,
              "auth_session encode should reject missing auth_session") &&
       ok;
  ok = expect_code(result, "supervisor_auth_session_missing",
                   "auth_session encode missing auth_session rejection code") &&
       ok;

  nlohmann::json missing_payload{{"config", sample_config()},
                                 {"native_start_mode", "auth_session"}};
  SupervisorStartPayload decoded;
  result = decode_vpn_supervisor_payload(missing_payload, &decoded);
  std::string text = result_text(result);
  ok = expect(!result.ok,
              "auth_session decode should reject missing auth_session") &&
       ok;
  ok = expect_code(result, "supervisor_auth_session_missing",
                   "auth_session decode missing auth_session rejection code") &&
       ok;
  ok = expect(text.find(MOCK_PASSWORD) == std::string::npos,
              "missing auth_session error must not leak config password") &&
       ok;

  auto invalid_payload = missing_payload;
  invalid_payload["auth_session"] = nlohmann::json{
      {"schema_version", 1},
      {"cookie_header", "webvpn=super-cookie-secret"}};
  result = decode_vpn_supervisor_payload(invalid_payload, &decoded);
  text = result_text(result);
  ok = expect(!result.ok,
              "auth_session decode should reject invalid auth_session") &&
       ok;
  ok = expect_code(result, "supervisor_auth_session_invalid",
                   "auth_session decode invalid auth_session rejection code") &&
       ok;
  ok = expect(text.find("super-cookie-secret") == std::string::npos,
              "invalid auth_session error must not leak cookie") &&
       ok;
  ok = expect(text.find(MOCK_PASSWORD) == std::string::npos,
              "invalid auth_session error must not leak config password") &&
       ok;
  return ok;
}

bool auth_session_decode_allows_empty_top_level_password_and_clears_secrets() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::decode_vpn_supervisor_payload;

  auto config = sample_config();
  config.password = MOCK_PASSWORD;
  const auto session = sample_auth_session();
  nlohmann::json payload{{"config", config},
                         {"native_start_mode", "auth_session"},
                         {"password", ""},
                         {"auth_session",
                          ecnuvpn::vpn_engine::protocol::to_json(session)}};

  SupervisorStartPayload decoded;
  const auto result = decode_vpn_supervisor_payload(payload, &decoded);

  bool ok = true;
  ok = expect(result.ok,
              "auth_session decode should allow empty top-level password") &&
       ok;
  ok = expect(decoded.native_start_mode == SupervisorStartMode::auth_session,
              "auth_session decode should preserve explicit mode") &&
       ok;
  ok = expect(decoded.password.empty(),
              "auth_session decode should clear empty top-level password") &&
       ok;
  ok = expect(decoded.config.password.empty(),
              "auth_session decode should clear config.password") &&
       ok;
  ok = expect(decoded.auth_session.has_value(),
              "auth_session decode should preserve auth_session") &&
       ok;
  ok = expect(decoded.auth_session.has_value() &&
                  auth_sessions_equal(*decoded.auth_session, session),
              "auth_session decode should preserve auth_session fields") &&
       ok;
  return ok;
}

bool unknown_mode_is_rejected() {
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::decode_vpn_supervisor_payload;

  nlohmann::json payload{{"config", sample_config()},
                         {"native_start_mode", "magic"},
                         {"password", MOCK_PASSWORD}};
  SupervisorStartPayload decoded;
  const auto result = decode_vpn_supervisor_payload(payload, &decoded);

  bool ok = true;
  ok = expect(!result.ok, "unknown native_start_mode should reject") && ok;
  ok = expect(result.code == "supervisor_start_mode_invalid",
              "unknown native_start_mode should use deterministic code") &&
       ok;
  ok = expect(result_text(result).find(MOCK_PASSWORD) ==
                  std::string::npos,
              "unknown mode error must not leak password") &&
       ok;
  return ok;
}

bool redacted_summary_excludes_password_and_cookie_secret_values() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::summarize_vpn_supervisor_payload;

  SupervisorStartPayload payload;
  payload.config = sample_config();
  payload.config.password = MOCK_PASSWORD;
  payload.config.server = "https://vpn.example.invalid/?token=server-secret";
  payload.auth_session = sample_auth_session();
  payload.auth_session->useragent = "agent webvpn token secret";
  payload.native_start_mode = SupervisorStartMode::auth_session;
  payload.home = "/home/student";
  payload.config_dir = "/home/student/.config/ecnu-vpn";

  const auto summary = summarize_vpn_supervisor_payload(payload).dump();

  bool ok = true;
  ok = expect(summary.find(MOCK_PASSWORD) == std::string::npos,
              "summary must not contain config password") &&
       ok;
  ok = expect(summary.find("super-cookie-secret") == std::string::npos,
              "summary must not contain cookie value") &&
       ok;
  ok = expect(summary.find("server-secret") == std::string::npos,
              "summary must not contain token-bearing config strings") &&
       ok;
  ok = expect(summary.find("agent webvpn token secret") == std::string::npos,
              "summary must not contain token-bearing auth strings") &&
       ok;
  ok = expect(summary.find("webvpn=") == std::string::npos,
              "summary must not contain webvpn cookie marker") &&
       ok;
  ok = expect(summary.find("token") == std::string::npos,
              "summary must not contain token marker from secrets") &&
       ok;
  ok = expect(summary.find(MOCK_PASSWORD) == std::string::npos,
              "summary must not contain password secret fragments") &&
       ok;
  return ok;
}

bool safe_payload_parse_decode_returns_fixed_error_without_leak() {
  using ecnuvpn::platform::parse_vpn_supervisor_payload;
  using ecnuvpn::platform::SupervisorStartPayload;

  const std::string secret = "webvpn=super-cookie-secret";
  const std::string malformed =
      "{\"config\":{\"server\":\"https://vpn.example.invalid\"},"
      "\"native_start_mode\":\"auth_session\",\"auth_session\":{\"cookie_header\":\"" +
      secret;

  SupervisorStartPayload decoded;
  auto result = parse_vpn_supervisor_payload(malformed, &decoded);

  bool ok = true;
  ok = expect(!result.ok, "malformed payload should not parse") && ok;
  ok = expect_code(result, "supervisor_payload_invalid",
                   "malformed payload should use fixed code") &&
       ok;
  ok = expect(result.message == "supervisor startup payload is invalid",
              "malformed payload should use fixed safe message") &&
       ok;
  ok = expect_no_leak(result, secret,
                      "malformed payload error must not leak stdin content") &&
       ok;

  nlohmann::json invalid_decode{{"native_start_mode", "auth_session"},
                                {"password", secret}};
  result = parse_vpn_supervisor_payload(invalid_decode.dump(), &decoded);
  ok = expect(!result.ok, "decode-invalid payload should fail") && ok;
  ok = expect_code(result, "supervisor_payload_invalid",
                   "decode-invalid payload should use fixed code") &&
       ok;
  ok = expect(result.message == "supervisor startup payload is invalid",
              "decode-invalid payload should use fixed safe message") &&
       ok;
  ok = expect_no_leak(result, secret,
                      "decode-invalid payload error must not leak stdin content") &&
       ok;
  return ok;
}

bool supervisor_payload_runner_dispatches_password_mode_to_password_path() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::run_vpn_supervisor_payload;

  SupervisorStartPayload payload;
  payload.config = sample_config();
  payload.config.vpn_engine = "openconnect";
  payload.password = MOCK_PASSWORD;
  payload.retry_limit = 4;
  payload.native_start_mode = SupervisorStartMode::password;

  bool password_called = false;
  bool auth_called = false;
  const int result = run_vpn_supervisor_payload(
      payload,
      [&](const ecnuvpn::Config &cfg, const std::string &password,
          int retry_limit) {
        password_called = true;
        return cfg.vpn_engine == "openconnect" &&
                       password == MOCK_PASSWORD &&
                       retry_limit == 4
                   ? 23
                   : 1;
      },
      [&](const ecnuvpn::Config &,
          const ecnuvpn::vpn_engine::protocol::NativeAuthSession &, int) {
        auth_called = true;
        return 1;
      });

  bool ok = true;
  ok = expect(result == 23,
              "password payload runner should return password path result") &&
       ok;
  ok = expect(password_called,
              "password payload runner should invoke password path") &&
       ok;
  ok = expect(!auth_called,
              "password payload runner should not invoke auth_session path") &&
       ok;
  return ok;
}

bool supervisor_payload_runner_dispatches_native_auth_session_mode() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::run_vpn_supervisor_payload;

  const auto session = sample_auth_session();
  SupervisorStartPayload payload;
  payload.config = sample_config();
  payload.config.password = "test-mock-must-not-be-used";
  payload.password = "test-mock-must-not-be-used";
  payload.auth_session = session;
  payload.retry_limit = -1;
  payload.native_start_mode = SupervisorStartMode::auth_session;

  bool password_called = false;
  bool auth_called = false;
  const int result = run_vpn_supervisor_payload(
      payload,
      [&](const ecnuvpn::Config &, const std::string &, int) {
        password_called = true;
        return 1;
      },
      [&](const ecnuvpn::Config &cfg,
          const ecnuvpn::vpn_engine::protocol::NativeAuthSession &auth_session,
          int retry_limit) {
        auth_called = true;
        return cfg.vpn_engine == "native" && cfg.password.empty() &&
                       auth_sessions_equal(auth_session, session) &&
                       retry_limit == -1
                   ? 42
                   : 1;
      });

  bool ok = true;
  ok = expect(result == 42,
              "auth_session payload runner should return auth path result") &&
       ok;
  ok = expect(auth_called,
              "auth_session payload runner should invoke auth_session path") &&
       ok;
  ok = expect(!password_called,
              "auth_session payload runner should not invoke password path") &&
       ok;
  return ok;
}

bool supervisor_payload_runner_rejects_auth_session_for_non_native_engine() {
  using ecnuvpn::platform::SupervisorStartMode;
  using ecnuvpn::platform::SupervisorStartPayload;
  using ecnuvpn::platform::run_vpn_supervisor_payload;

  SupervisorStartPayload payload;
  payload.config = sample_config();
  payload.config.vpn_engine = "openconnect";
  payload.auth_session = sample_auth_session();
  payload.native_start_mode = SupervisorStartMode::auth_session;

  bool password_called = false;
  bool auth_called = false;
  const int result = run_vpn_supervisor_payload(
      payload,
      [&](const ecnuvpn::Config &, const std::string &, int) {
        password_called = true;
        return 0;
      },
      [&](const ecnuvpn::Config &,
          const ecnuvpn::vpn_engine::protocol::NativeAuthSession &, int) {
        auth_called = true;
        return 0;
      });

  bool ok = true;
  ok = expect(result != 0,
              "auth_session payload runner should fail for non-native engine") &&
       ok;
  ok = expect(!password_called,
              "non-native auth_session payload should not invoke password path") &&
       ok;
  ok = expect(!auth_called,
              "non-native auth_session payload should not invoke auth path") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = legacy_payload_missing_mode_decodes_password_mode_and_preserves_password() &&
       ok;
  ok = password_mode_rejects_auth_session_and_missing_or_invalid_password() &&
       ok;
  ok = explicit_password_mode_roundtrip_preserves_password_without_auth_session() &&
       ok;
  ok = auth_session_roundtrip_omits_password_and_clears_config_password() && ok;
  ok = auth_session_mode_with_non_empty_password_is_rejected_without_leak() &&
       ok;
  ok = auth_session_mode_missing_or_invalid_auth_session_is_rejected_without_leak() &&
       ok;
  ok = auth_session_decode_allows_empty_top_level_password_and_clears_secrets() &&
       ok;
  ok = unknown_mode_is_rejected() && ok;
  ok = redacted_summary_excludes_password_and_cookie_secret_values() && ok;
  ok = safe_payload_parse_decode_returns_fixed_error_without_leak() && ok;
  ok = supervisor_payload_runner_dispatches_password_mode_to_password_path() &&
       ok;
  ok = supervisor_payload_runner_dispatches_native_auth_session_mode() && ok;
  ok = supervisor_payload_runner_rejects_auth_session_for_non_native_engine() &&
       ok;

  return ok ? 0 : 1;
}
