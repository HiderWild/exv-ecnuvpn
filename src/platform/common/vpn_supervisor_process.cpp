#include "platform/common/vpn_supervisor_process.hpp"

#include "vpn_engine/protocol/native_auth_session_json.hpp"

#include <cctype>
#include <utility>

namespace ecnuvpn {
namespace platform {
namespace {

vpn_engine::ValidationResult invalid(std::string code, std::string message) {
  vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

vpn_engine::ValidationResult invalid_startup_payload() {
  return invalid("supervisor_payload_invalid",
                 "supervisor startup payload is invalid");
}

const char *mode_to_string(SupervisorStartMode mode) {
  switch (mode) {
  case SupervisorStartMode::password:
    return "password";
  case SupervisorStartMode::auth_session:
    return "auth_session";
  }
  return "";
}

vpn_engine::ValidationResult parse_mode(const nlohmann::json &payload,
                                        SupervisorStartMode *mode) {
  if (!mode)
    return invalid("supervisor_start_mode_output_missing",
                   "supervisor start mode output must not be null");

  if (!payload.contains("native_start_mode")) {
    *mode = SupervisorStartMode::password;
    return vpn_engine::ValidationResult{};
  }

  if (!payload.at("native_start_mode").is_string()) {
    return invalid("supervisor_start_mode_invalid",
                   "supervisor native_start_mode is invalid");
  }

  const std::string mode_value =
      payload.at("native_start_mode").get<std::string>();
  if (mode_value == "password") {
    *mode = SupervisorStartMode::password;
    return vpn_engine::ValidationResult{};
  }
  if (mode_value == "auth_session") {
    *mode = SupervisorStartMode::auth_session;
    return vpn_engine::ValidationResult{};
  }

  return invalid("supervisor_start_mode_invalid",
                 "supervisor native_start_mode is invalid");
}

bool get_optional_string(const nlohmann::json &object, const char *field,
                         std::string *out) {
  if (!out)
    return false;
  out->clear();
  if (!object.contains(field))
    return true;
  if (!object.at(field).is_string())
    return false;
  *out = object.at(field).get<std::string>();
  return true;
}

bool get_optional_int(const nlohmann::json &object, const char *field,
                      int *out) {
  if (!out)
    return false;
  *out = 0;
  if (!object.contains(field))
    return true;
  if (!object.at(field).is_number_integer())
    return false;
  try {
    *out = object.at(field).get<int>();
  } catch (const nlohmann::json::exception &) {
    return false;
  }
  return true;
}

vpn_engine::ValidationResult decode_common_fields(const nlohmann::json &payload,
                                                  SupervisorStartPayload *out) {
  if (!payload.contains("config") || !payload.at("config").is_object()) {
    return invalid("supervisor_config_invalid",
                   "supervisor config must be a JSON object");
  }

  try {
    out->config = payload.at("config").get<Config>();
  } catch (const nlohmann::json::exception &) {
    return invalid("supervisor_config_invalid",
                   "supervisor config must be a valid config object");
  }

  if (!get_optional_int(payload, "retry_limit", &out->retry_limit)) {
    return invalid("supervisor_retry_limit_invalid",
                   "supervisor retry_limit must be an integer");
  }
  if (!get_optional_string(payload, "home", &out->home)) {
    return invalid("supervisor_home_invalid",
                   "supervisor home must be a string");
  }
  if (!get_optional_string(payload, "config_dir", &out->config_dir)) {
    return invalid("supervisor_config_dir_invalid",
                   "supervisor config_dir must be a string");
  }

  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult validate_password_mode_for_encode(
    const SupervisorStartPayload &payload) {
  if (payload.auth_session.has_value()) {
    return invalid("supervisor_auth_session_forbidden",
                   "supervisor password mode must not include auth_session");
  }
  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult validate_auth_session_mode_for_encode(
    const SupervisorStartPayload &payload) {
  if (!payload.password.empty()) {
    return invalid("supervisor_password_forbidden",
                   "supervisor auth_session mode must not include password");
  }
  if (!payload.auth_session.has_value()) {
    return invalid("supervisor_auth_session_missing",
                   "supervisor auth_session mode requires auth_session");
  }
  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult decode_password_mode(const nlohmann::json &payload,
                                                  SupervisorStartPayload *out) {
  if (!payload.contains("password") || !payload.at("password").is_string()) {
    return invalid("supervisor_password_missing",
                   "supervisor password mode requires password");
  }
  if (payload.contains("auth_session")) {
    return invalid("supervisor_auth_session_forbidden",
                   "supervisor password mode must not include auth_session");
  }

  out->password = payload.at("password").get<std::string>();
  out->auth_session.reset();
  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult
decode_auth_session_mode(const nlohmann::json &payload,
                         SupervisorStartPayload *out) {
  if (payload.contains("password")) {
    if (!payload.at("password").is_string()) {
      return invalid("supervisor_password_forbidden",
                     "supervisor auth_session mode must not include password");
    }
    if (!payload.at("password").get<std::string>().empty()) {
      return invalid("supervisor_password_forbidden",
                     "supervisor auth_session mode must not include password");
    }
  }

  if (!payload.contains("auth_session") ||
      !payload.at("auth_session").is_object()) {
    return invalid("supervisor_auth_session_missing",
                   "supervisor auth_session mode requires auth_session");
  }

  vpn_engine::protocol::NativeAuthSession session;
  const auto parsed =
      vpn_engine::protocol::from_json(payload.at("auth_session"), &session);
  if (!parsed.ok) {
    return invalid("supervisor_auth_session_invalid",
                   "supervisor auth_session is invalid");
  }

  out->password.clear();
  out->config.password.clear();
  out->auth_session = std::move(session);
  return vpn_engine::ValidationResult{};
}

nlohmann::json summarize_config(const Config &config) {
  return nlohmann::json{{"server", config.server},
                        {"username", config.username},
                        {"password_present", !config.password.empty()},
                        {"mtu", config.mtu},
                        {"useragent", config.useragent},
                        {"disable_dtls", config.disable_dtls},
                        {"remember_password", config.remember_password},
                        {"routes_count", config.routes.size()},
                        {"extra_args_count", config.extra_args.size()},
                        {"log_file", config.log_file},
                        {"webui_port", config.webui_port},
                        {"webui_bind", config.webui_bind},
                        {"webui_enabled", config.webui_enabled},
                        {"vpn_engine", config.vpn_engine},
                        {"openconnect_runtime", config.openconnect_runtime},
                        {"windows_tunnel_driver", config.windows_tunnel_driver},
                        {"windows_tap_interface",
                         config.windows_tap_interface.empty()
                             ? nlohmann::json(nullptr)
                             : nlohmann::json("[redacted]")},
                        {"auto_reconnect", config.auto_reconnect},
                        {"minimal_mode", config.minimal_mode},
                        {"service_install_prompt_seen",
                         config.service_install_prompt_seen},
                        {"minimal_install_service_before_connect",
                         config.minimal_install_service_before_connect}};
}

std::string to_ascii_lower(const std::string &input) {
  std::string out;
  out.reserve(input.size());
  for (unsigned char ch : input)
    out.push_back(static_cast<char>(std::tolower(ch)));
  return out;
}

bool contains_secret_marker(const std::string &value) {
  const std::string lower = to_ascii_lower(value);
  for (const char *marker :
       {"webvpn", "cookie", "token", "secret", "password", "saml", "csrf"}) {
    if (lower.find(marker) != std::string::npos)
      return true;
  }
  return false;
}

void redact_secret_strings(nlohmann::json *value) {
  if (!value)
    return;

  if (value->is_string()) {
    const std::string text = value->get<std::string>();
    if (contains_secret_marker(text))
      *value = "[redacted]";
    return;
  }

  if (value->is_array()) {
    for (auto &entry : *value)
      redact_secret_strings(&entry);
    return;
  }

  if (value->is_object()) {
    for (auto &entry : value->items())
      redact_secret_strings(&entry.value());
  }
}

} // namespace

vpn_engine::ValidationResult
encode_vpn_supervisor_payload(const SupervisorStartPayload &payload,
                              nlohmann::json *out) {
  if (!out) {
    return invalid("supervisor_payload_output_missing",
                   "supervisor payload output must not be null");
  }

  if (payload.native_start_mode == SupervisorStartMode::password) {
    const auto validation = validate_password_mode_for_encode(payload);
    if (!validation.ok)
      return validation;

    *out = nlohmann::json{{"config", payload.config},
                          {"password", payload.password},
                          {"retry_limit", payload.retry_limit},
                          {"home", payload.home},
                          {"config_dir", payload.config_dir},
                          {"native_start_mode",
                           mode_to_string(payload.native_start_mode)}};
    return vpn_engine::ValidationResult{};
  }

  if (payload.native_start_mode == SupervisorStartMode::auth_session) {
    const auto validation = validate_auth_session_mode_for_encode(payload);
    if (!validation.ok)
      return validation;

    Config config = payload.config;
    config.password.clear();
    nlohmann::json config_json = config;
    config_json.erase("password");
    *out = nlohmann::json{
        {"config", std::move(config_json)},
        {"retry_limit", payload.retry_limit},
        {"home", payload.home},
        {"config_dir", payload.config_dir},
        {"native_start_mode", mode_to_string(payload.native_start_mode)},
        {"auth_session", vpn_engine::protocol::to_json(*payload.auth_session)}};
    return vpn_engine::ValidationResult{};
  }

  return invalid("supervisor_start_mode_invalid",
                 "supervisor native_start_mode is invalid");
}

vpn_engine::ValidationResult
decode_vpn_supervisor_payload(const nlohmann::json &payload,
                              SupervisorStartPayload *out) {
  if (!out) {
    return invalid("supervisor_payload_output_missing",
                   "supervisor payload output must not be null");
  }
  if (!payload.is_object()) {
    return invalid("supervisor_payload_invalid",
                   "supervisor payload must be a JSON object");
  }

  SupervisorStartPayload parsed;
  auto validation = parse_mode(payload, &parsed.native_start_mode);
  if (!validation.ok)
    return validation;

  validation = decode_common_fields(payload, &parsed);
  if (!validation.ok)
    return validation;

  if (parsed.native_start_mode == SupervisorStartMode::password) {
    validation = decode_password_mode(payload, &parsed);
  } else if (parsed.native_start_mode == SupervisorStartMode::auth_session) {
    validation = decode_auth_session_mode(payload, &parsed);
  } else {
    validation = invalid("supervisor_start_mode_invalid",
                         "supervisor native_start_mode is invalid");
  }
  if (!validation.ok)
    return validation;

  *out = std::move(parsed);
  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult
parse_vpn_supervisor_payload(const std::string &payload,
                             SupervisorStartPayload *out) {
  if (!out) {
    return invalid("supervisor_payload_output_missing",
                   "supervisor payload output must not be null");
  }

  nlohmann::json parsed =
      nlohmann::json::parse(payload, nullptr, /*allow_exceptions=*/false);
  if (parsed.is_discarded())
    return invalid_startup_payload();

  const auto decoded = decode_vpn_supervisor_payload(parsed, out);
  if (!decoded.ok)
    return invalid_startup_payload();

  return vpn_engine::ValidationResult{};
}

nlohmann::json
summarize_vpn_supervisor_payload(const SupervisorStartPayload &payload) {
  nlohmann::json summary{
      {"native_start_mode", mode_to_string(payload.native_start_mode)},
      {"retry_limit", payload.retry_limit},
      {"home_present", !payload.home.empty()},
      {"config_dir_present", !payload.config_dir.empty()},
      {"password_present", !payload.password.empty()},
      {"config", summarize_config(payload.config)}};

  if (payload.auth_session.has_value()) {
    summary["auth_session"] =
        vpn_engine::protocol::summarize_native_auth_session(
            *payload.auth_session);
  } else {
    summary["auth_session_present"] = false;
  }

  redact_secret_strings(&summary);
  return summary;
}

int run_vpn_supervisor_payload(
    const SupervisorStartPayload &payload,
    const SupervisorPasswordRunner &password_runner,
    const SupervisorAuthSessionRunner &auth_session_runner) {
  if (payload.native_start_mode == SupervisorStartMode::password) {
    if (!password_runner)
      return 1;
    return password_runner(payload.config, payload.password,
                           payload.retry_limit);
  }

  if (payload.native_start_mode == SupervisorStartMode::auth_session) {
    if (payload.config.vpn_engine != "native" || !payload.auth_session ||
        !auth_session_runner) {
      return 1;
    }

    Config config = payload.config;
    config.password.clear();
    return auth_session_runner(config, *payload.auth_session,
                               payload.retry_limit);
  }

  return 1;
}

} // namespace platform
} // namespace ecnuvpn
