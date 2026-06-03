#include "app_api_native_orchestration.hpp"
#include "vpn_engine/protocol/native_auth_session_json.hpp"

#include <chrono>
#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

ecnuvpn::vpn_engine::ValidationResult invalid(std::string code,
                                              std::string message) {
  ecnuvpn::vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

ecnuvpn::Config sample_config(std::string engine = "native") {
  ecnuvpn::Config config;
  config.server = "https://vpn.example.invalid";
  config.username = "student@example.invalid";
  config.password = "stored-config-password-secret";
  config.mtu = 1280;
  config.useragent = "ECNU-VPN app-api orchestration test";
  config.disable_dtls = true;
  config.remember_password = false;
  config.routes = {"10.0.0.0/8", "192.0.2.1"};
  config.extra_args = {"--test-arg"};
  config.log_file = "orchestration-test.log";
  config.webui_port = 18081;
  config.webui_bind = "127.0.0.1";
  config.webui_enabled = false;
  config.vpn_engine = std::move(engine);
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
  session.useragent = "ECNU-VPN app-api auth-session test";
  session.cookie_header = "webvpn=super-cookie-secret";
  session.selected_group = "student";
  session.auth_method = "password";
  session.created_at =
      std::chrono::system_clock::time_point{std::chrono::milliseconds{
          1712345678123LL}};
  session.diagnostics["cookie_present"] = "true";
  return session;
}

std::string result_text(const ecnuvpn::vpn_engine::ValidationResult &result) {
  return result.code + " " + result.message;
}

bool request_does_not_contain_secret(const nlohmann::json &request,
                                     const std::string &secret,
                                     const char *message) {
  return expect(request.dump().find(secret) == std::string::npos, message);
}

bool native_auth_failure_does_not_call_helper_start() {
  using ecnuvpn::app_api::NativeAuthFirstDeps;
  using ecnuvpn::app_api::NativeAuthFirstInputs;
  using ecnuvpn::app_api::orchestrate_native_auth_first;

  NativeAuthFirstInputs inputs;
  inputs.config = sample_config();
  inputs.password = "top-level-password-secret";
  inputs.home = "C:/Users/student";
  inputs.config_dir = "C:/Users/student/.ecnu-vpn";
  inputs.retry_limit = 3;

  int auth_calls = 0;
  int helper_calls = 0;
  NativeAuthFirstDeps deps;
  deps.authenticate =
      [&](const ecnuvpn::Config &cfg, const std::string &password,
          ecnuvpn::vpn_engine::protocol::NativeAuthSession *session) {
        ++auth_calls;
        (void)cfg;
        (void)password;
        (void)session;
        return invalid("native_auth_failed", "native auth rejected");
      };
  deps.send_helper_start = [&](const nlohmann::json &) {
    ++helper_calls;
    return ecnuvpn::vpn_engine::ValidationResult{};
  };

  const auto result = orchestrate_native_auth_first(inputs, deps);

  bool ok = true;
  ok = expect(!result.ok, "native auth failure should fail orchestration") && ok;
  ok = expect(result.code == "native_auth_failed",
              "native auth failure code should be preserved") &&
       ok;
  ok = expect(result.message == "native auth rejected",
              "native auth failure message should be preserved") &&
       ok;
  ok = expect(auth_calls == 1, "native auth should be attempted once") && ok;
  ok = expect(helper_calls == 0,
              "helper start must not be called after native auth failure") &&
       ok;
  ok = expect(result_text(result).find("top-level-password-secret") ==
                  std::string::npos,
              "native auth failure result must not leak password") &&
       ok;
  return ok;
}

bool native_auth_success_helper_request_omits_password() {
  using ecnuvpn::app_api::NativeAuthFirstDeps;
  using ecnuvpn::app_api::NativeAuthFirstInputs;
  using ecnuvpn::app_api::orchestrate_native_auth_first;

  NativeAuthFirstInputs inputs;
  inputs.config = sample_config();
  inputs.password = "top-level-password-secret";
  inputs.home = "/home/student";
  inputs.config_dir = "/home/student/.config/ecnu-vpn";
  inputs.retry_limit = 4;

  const auto session = sample_auth_session();
  nlohmann::json sent_request;
  std::vector<std::string> calls;
  NativeAuthFirstDeps deps;
  deps.authenticate =
      [&](const ecnuvpn::Config &, const std::string &password,
          ecnuvpn::vpn_engine::protocol::NativeAuthSession *out) {
        calls.push_back("auth");
        if (password == "top-level-password-secret" && out)
          *out = session;
        return ecnuvpn::vpn_engine::ValidationResult{};
      };
  deps.send_helper_start = [&](const nlohmann::json &request) {
    calls.push_back("helper");
    sent_request = request;
    return ecnuvpn::vpn_engine::ValidationResult{};
  };

  const auto result = orchestrate_native_auth_first(inputs, deps);

  bool ok = true;
  ok = expect(result.ok, "native auth success should send helper request") && ok;
  ok = expect(calls == std::vector<std::string>({"auth", "helper"}),
              "native auth should happen before helper start") &&
       ok;
  ok = expect(sent_request.value("action", std::string()) == "start",
              "helper request should be a start action") &&
       ok;
  ok = expect(sent_request.value("native_start_mode", std::string()) ==
                  "auth_session",
              "native helper request should use auth_session mode") &&
       ok;
  ok = expect(sent_request.contains("auth_session") &&
                  sent_request.at("auth_session").is_object(),
              "native helper request should include auth_session object") &&
       ok;
  ok = expect(!sent_request.contains("password"),
              "native helper request must omit top-level password") &&
       ok;
  ok = expect(!sent_request.at("config").contains("password"),
              "native helper request config must omit password") &&
       ok;
  ok = expect(sent_request.value("retry_limit", 0) == 4,
              "native helper request should preserve retry limit") &&
       ok;
  ok = expect(sent_request.value("home", std::string()) == "/home/student",
              "native helper request should preserve home") &&
       ok;
  ok = expect(sent_request.value("config_dir", std::string()) ==
                  "/home/student/.config/ecnu-vpn",
              "native helper request should preserve config_dir") &&
       ok;
  ok = request_does_not_contain_secret(
           sent_request, "top-level-password-secret",
           "native helper request must not leak top-level password") &&
       ok;
  ok = request_does_not_contain_secret(
           sent_request, "stored-config-password-secret",
           "native helper request must not leak config password") &&
       ok;
  return ok;
}

bool legacy_openconnect_path_unchanged() {
  using ecnuvpn::app_api::NativeAuthFirstDeps;
  using ecnuvpn::app_api::NativeAuthFirstInputs;
  using ecnuvpn::app_api::orchestrate_native_auth_first;

  NativeAuthFirstInputs inputs;
  inputs.config = sample_config("legacy_openconnect");
  inputs.password = "top-level-password-secret";
  inputs.home = "/home/student";
  inputs.config_dir = "/home/student/.config/ecnu-vpn";
  inputs.retry_limit = 5;

  int auth_calls = 0;
  nlohmann::json sent_request;
  NativeAuthFirstDeps deps;
  deps.authenticate =
      [&](const ecnuvpn::Config &, const std::string &,
          ecnuvpn::vpn_engine::protocol::NativeAuthSession *) {
        ++auth_calls;
        return invalid("unexpected_auth", "legacy path called native auth");
      };
  deps.send_helper_start = [&](const nlohmann::json &request) {
    sent_request = request;
    return ecnuvpn::vpn_engine::ValidationResult{};
  };

  const auto result = orchestrate_native_auth_first(inputs, deps);

  bool ok = true;
  ok = expect(result.ok, "legacy path should send password helper request") &&
       ok;
  ok = expect(auth_calls == 0,
              "legacy path must not call native auth runner") &&
       ok;
  ok = expect(sent_request.value("action", std::string()) == "start",
              "legacy helper request should be a start action") &&
       ok;
  ok = expect(sent_request.value("native_start_mode", std::string()) ==
                  "password",
              "legacy helper request should use password mode") &&
       ok;
  ok = expect(sent_request.value("password", std::string()) ==
                  "top-level-password-secret",
              "legacy helper request should keep top-level password") &&
       ok;
  ok = expect(sent_request.at("config").value("password", std::string()) ==
                  "stored-config-password-secret",
              "legacy helper request should preserve config password behavior") &&
       ok;
  ok = expect(!sent_request.contains("auth_session"),
              "legacy helper request should not include auth_session") &&
       ok;
  ok = expect(sent_request.value("retry_limit", 0) == 5,
              "legacy helper request should preserve retry limit") &&
       ok;
  return ok;
}

bool service_unavailable_short_circuits_before_native_auth() {
  using ecnuvpn::app_api::NativeAuthFirstDeps;
  using ecnuvpn::app_api::NativeAuthFirstInputs;
  using ecnuvpn::app_api::orchestrate_native_auth_first;

  NativeAuthFirstInputs inputs;
  inputs.config = sample_config();
  inputs.password = "top-level-password-secret";
  inputs.allow_direct_fallback = false;

  int service_checks = 0;
  int auth_calls = 0;
  int helper_calls = 0;
  NativeAuthFirstDeps deps;
  deps.ensure_service_available_or_start_oneshot = [&] {
    ++service_checks;
    return invalid("helper_unavailable", "helper service unavailable");
  };
  deps.authenticate =
      [&](const ecnuvpn::Config &, const std::string &,
          ecnuvpn::vpn_engine::protocol::NativeAuthSession *) {
        ++auth_calls;
        return ecnuvpn::vpn_engine::ValidationResult{};
      };
  deps.send_helper_start = [&](const nlohmann::json &) {
    ++helper_calls;
    return ecnuvpn::vpn_engine::ValidationResult{};
  };

  const auto result = orchestrate_native_auth_first(inputs, deps);

  bool ok = true;
  ok = expect(!result.ok, "service unavailable should fail orchestration") && ok;
  ok = expect(result.code == "helper_unavailable",
              "service unavailable code should be preserved") &&
       ok;
  ok = expect(service_checks == 1, "service should be checked once") && ok;
  ok = expect(auth_calls == 0,
              "non-fallback service failure should short-circuit before auth") &&
       ok;
  ok = expect(helper_calls == 0,
              "service failure should not call helper start") &&
       ok;
  return ok;
}

bool fallback_native_auth_happens_before_helper_start() {
  using ecnuvpn::app_api::NativeAuthFirstDeps;
  using ecnuvpn::app_api::NativeAuthFirstInputs;
  using ecnuvpn::app_api::orchestrate_native_auth_first;

  NativeAuthFirstInputs inputs;
  inputs.config = sample_config();
  inputs.password = "top-level-password-secret";
  inputs.allow_direct_fallback = true;

  std::vector<std::string> calls;
  NativeAuthFirstDeps deps;
  deps.ensure_service_available_or_start_oneshot = [&] {
    calls.push_back("service");
    return ecnuvpn::vpn_engine::ValidationResult{};
  };
  deps.authenticate =
      [&](const ecnuvpn::Config &, const std::string &,
          ecnuvpn::vpn_engine::protocol::NativeAuthSession *out) {
        calls.push_back("auth");
        if (out)
          *out = sample_auth_session();
        return ecnuvpn::vpn_engine::ValidationResult{};
      };
  deps.send_helper_start = [&](const nlohmann::json &) {
    calls.push_back("helper");
    return ecnuvpn::vpn_engine::ValidationResult{};
  };

  const auto result = orchestrate_native_auth_first(inputs, deps);

  bool ok = true;
  ok = expect(result.ok, "fallback native path should start helper") && ok;
  ok = expect(calls == std::vector<std::string>({"auth", "service", "helper"}),
              "fallback path should authenticate before oneshot/helper start") &&
       ok;
  return ok;
}

bool native_user_mode_auth_request_matches_engine_options() {
  ecnuvpn::Config cfg = sample_config();
  cfg.server = "https://vpn.example.invalid:4443/group";
  cfg.username = "native-user";
  cfg.useragent = "ECNU-VPN native adapter test";
  cfg.mtu = 9999;
  cfg.auto_reconnect = true;

  ecnuvpn::vpn_engine::protocol::NativeAuthRequest request;
  const auto result = ecnuvpn::app_api::build_native_user_mode_auth_request(
      cfg, "plaintext-secret", &request);

  bool ok = true;
  ok = expect(result.ok, "native auth request build should succeed") && ok;
  ok = expect(request.options.server.scheme == "https",
              "native auth request should parse https scheme") &&
       ok;
  ok = expect(request.options.server.host == "vpn.example.invalid",
              "native auth request should parse host") &&
       ok;
  ok = expect(request.options.server.port == 4443,
              "native auth request should parse explicit port") &&
       ok;
  ok = expect(request.options.server.base_path == "/group",
              "native auth request should parse base path") &&
       ok;
  ok = expect(request.options.username == "native-user",
              "native auth request should use config username") &&
       ok;
  ok = expect(request.options.password == "plaintext-secret",
              "native auth request should use supplied plaintext password") &&
       ok;
  ok = expect(request.options.useragent == "ECNU-VPN native adapter test",
              "native auth request should use config useragent") &&
       ok;
  ok = expect(request.options.disable_dtls,
              "native auth request should force CSTP/TLS mode") &&
       ok;
  ok = expect(!request.options.auto_reconnect,
              "native auth request should disable reconnect") &&
       ok;
  ok = expect(request.options.max_reconnects == 0,
              "native auth request should set zero reconnect attempts") &&
       ok;
  ok = expect(request.options.mtu_fallback == 1290,
              "native auth request should use safe MTU fallback") &&
       ok;
  return ok;
}

bool native_user_mode_auth_rejects_bad_server_without_helper_start() {
  using ecnuvpn::app_api::NativeAuthFirstDeps;
  using ecnuvpn::app_api::NativeAuthFirstInputs;
  using ecnuvpn::app_api::authenticate_native_user_mode;
  using ecnuvpn::app_api::orchestrate_native_auth_first;

  NativeAuthFirstInputs inputs;
  inputs.config = sample_config();
  inputs.config.server = "http://vpn.example.invalid";
  inputs.password = "top-level-password-secret";
  inputs.allow_direct_fallback = true;

  int helper_calls = 0;
  int service_calls = 0;
  NativeAuthFirstDeps deps;
  deps.authenticate =
      [](const ecnuvpn::Config &cfg, const std::string &password,
         ecnuvpn::vpn_engine::protocol::NativeAuthSession *session) {
        return authenticate_native_user_mode(cfg, password, session);
      };
  deps.ensure_service_available_or_start_oneshot = [&] {
    ++service_calls;
    return ecnuvpn::vpn_engine::ValidationResult{};
  };
  deps.send_helper_start = [&](const nlohmann::json &) {
    ++helper_calls;
    return ecnuvpn::vpn_engine::ValidationResult{};
  };

  const auto result = orchestrate_native_auth_first(inputs, deps);

  bool ok = true;
  ok = expect(!result.ok, "bad native auth server should fail") && ok;
  ok = expect(result.code == "url_scheme_invalid",
              "bad native auth server should preserve URL error code") &&
       ok;
  ok = expect(helper_calls == 0,
              "bad native auth server must not send helper start") &&
       ok;
  ok = expect(service_calls == 0,
              "fallback native auth failure must happen before oneshot start") &&
       ok;
  ok = expect(result_text(result).find("top-level-password-secret") ==
                  std::string::npos,
              "bad native auth result must not leak password") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = native_auth_failure_does_not_call_helper_start() && ok;
  ok = native_auth_success_helper_request_omits_password() && ok;
  ok = legacy_openconnect_path_unchanged() && ok;
  ok = service_unavailable_short_circuits_before_native_auth() && ok;
  ok = fallback_native_auth_happens_before_helper_start() && ok;
  ok = native_user_mode_auth_request_matches_engine_options() && ok;
  ok = native_user_mode_auth_rejects_bad_server_without_helper_start() && ok;

  return ok ? 0 : 1;
}
