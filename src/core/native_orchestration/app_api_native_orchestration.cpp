#include "app_api_native_orchestration.hpp"

#include "platform/common/vpn_supervisor_process.hpp"
#include "vpn_engine/native_engine.hpp"

#include <algorithm>
#include <exception>
#include <memory>
#include <utility>

namespace ecnuvpn {
namespace app_api {
namespace {

vpn_engine::ValidationResult invalid(std::string code, std::string message) {
  vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

bool is_native_engine(const Config &cfg) { return cfg.vpn_engine == "native"; }

vpn_engine::ValidationResult maybe_ensure_service(const NativeAuthFirstDeps &deps) {
  if (!deps.ensure_service_available_or_start_oneshot)
    return vpn_engine::ValidationResult{};
  return deps.ensure_service_available_or_start_oneshot();
}

vpn_engine::ValidationResult authenticate_native(
    const NativeAuthFirstInputs &inputs, const NativeAuthFirstDeps &deps,
    vpn_engine::protocol::NativeAuthSession *session) {
  if (!deps.authenticate) {
    return invalid("native_auth_runner_missing",
                   "native auth runner is not configured");
  }
  return deps.authenticate(inputs.config, inputs.password, session);
}

vpn_engine::ValidationResult send_helper(const nlohmann::json &request,
                                         const NativeAuthFirstDeps &deps) {
  if (!deps.send_helper_start) {
    return invalid("helper_start_sender_missing",
                   "helper start sender is not configured");
  }
  return deps.send_helper_start(request);
}

vpn_engine::ValidationResult
build_helper_start_request_for_connect(
    const Config &cfg, const std::string &password,
    const vpn_engine::protocol::NativeAuthSession *auth_session,
    const std::string &home, const std::string &config_dir, int retry_limit,
    nlohmann::json *out) {
  if (!out) {
    return invalid("helper_start_request_output_missing",
                   "helper start request output must not be null");
  }

  platform::SupervisorStartPayload payload;
  payload.config = cfg;
  payload.retry_limit = retry_limit;
  payload.home = home;
  payload.config_dir = config_dir;
  if (auth_session) {
    payload.native_start_mode = platform::SupervisorStartMode::auth_session;
    payload.auth_session = *auth_session;
  } else {
    payload.native_start_mode = platform::SupervisorStartMode::password;
    payload.password = password;
  }

  nlohmann::json encoded;
  const auto result = platform::encode_vpn_supervisor_payload(payload, &encoded);
  if (!result.ok)
    return result;

  encoded["action"] = "start";
  *out = std::move(encoded);
  return vpn_engine::ValidationResult{};
}

} // namespace

vpn_engine::ValidationResult
orchestrate_native_auth_first(const NativeAuthFirstInputs &inputs,
                              const NativeAuthFirstDeps &deps) {
  if (!is_native_engine(inputs.config)) {
    nlohmann::json request;
    auto built = build_helper_start_request_for_connect(
        inputs.config, inputs.password, nullptr, inputs.home, inputs.config_dir,
        inputs.retry_limit, &request);
    if (!built.ok)
      return built;
    return send_helper(request, deps);
  }

  (void)deps;
  return invalid("native_controller_required",
                 "Native engine connections must use TunnelController and Helper V2.");
}

vpn_engine::ValidationResult build_native_user_mode_auth_request(
    const Config &cfg, const std::string &password,
    vpn_engine::protocol::NativeAuthRequest *out) {
  if (!out) {
    return invalid("native_auth_request_output_missing",
                   "native auth request output must not be null");
  }

  vpn_engine::protocol::ParsedVpnUrl parsed;
  const auto parsed_result = vpn_engine::protocol::parse_vpn_url(cfg.server, &parsed);
  if (!parsed_result.ok)
    return parsed_result;

  vpn_engine::protocol::ProtocolSessionOptions options;
  options.server = std::move(parsed);
  options.username = cfg.username;
  options.password = password;
  options.useragent = cfg.useragent;
  options.disable_dtls = true;
  options.auto_reconnect = false;
  options.max_reconnects = 0;
  options.mtu_fallback = (cfg.mtu >= 576 && cfg.mtu <= 1500) ? cfg.mtu : 1290;

  out->options = std::move(options);
  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult authenticate_native_user_mode(
    const Config &cfg, const std::string &password,
    vpn_engine::protocol::NativeAuthSession *session) {
  vpn_engine::protocol::NativeAuthRequest request;
  auto built = build_native_user_mode_auth_request(cfg, password, &request);
  if (!built.ok)
    return built;

  auto dependencies = vpn_engine::default_native_engine_dependencies();
  if (!dependencies.transport_factory) {
    std::fill(request.options.password.begin(), request.options.password.end(),
              '\0');
    request.options.password.clear();
    return invalid("native_transport_unimplemented",
                   "Native engine transport factory is not configured.");
  }

  std::unique_ptr<vpn_engine::protocol::ProtocolTransport> transport;
  try {
    transport = dependencies.transport_factory();
  } catch (const std::exception &e) {
    std::fill(request.options.password.begin(), request.options.password.end(),
              '\0');
    request.options.password.clear();
    return invalid("native_transport_factory_failed", e.what());
  } catch (...) {
    std::fill(request.options.password.begin(), request.options.password.end(),
              '\0');
    request.options.password.clear();
    return invalid("native_transport_factory_failed",
                   "Native transport factory failed.");
  }

  if (!transport) {
    std::fill(request.options.password.begin(), request.options.password.end(),
              '\0');
    request.options.password.clear();
    return invalid("native_transport_unimplemented",
                   "Native engine transport factory is not configured.");
  }

  vpn_engine::protocol::NativeAuthenticator authenticator(transport.get());
  auto authenticated = authenticator.authenticate(request, session);
  std::fill(request.options.password.begin(), request.options.password.end(),
            '\0');
  request.options.password.clear();
  transport->disconnect();
  return authenticated;
}

} // namespace app_api
} // namespace ecnuvpn
