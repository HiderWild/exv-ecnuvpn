#include "vpn_engine/native_handshake_job.hpp"

#include "vpn_engine/protocol/url.hpp"

#include <exception>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {
namespace {

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

ValidationResult cancelled() {
  return invalid("cancelled", "Native handshake cancelled");
}

ValidationResult native_transport_unimplemented() {
  return invalid("native_transport_unimplemented",
                 "Native transport is not available on this platform.");
}

protocol::ProtocolSessionOptions make_protocol_options(
    const VpnEngineConfig &config,
    const protocol::ParsedVpnUrl &parsed,
    const NativeVpnEngineDependencies &dependencies) {
  protocol::ProtocolSessionOptions options;
  options.server = parsed;
  options.username = config.username;
  options.password = config.password;
  options.useragent = config.useragent;
  options.auth_group = config.auth_group;
  options.csd_wrapper = config.csd_wrapper;
  options.disable_dtls = config.disable_dtls;
  options.auto_reconnect = config.auto_reconnect;
  options.max_reconnects = config.auto_reconnect ? 1 : 0;
  options.mtu_fallback =
      (config.mtu >= 576 && config.mtu <= 1500) ? config.mtu : 1290;
  options.auth_interaction_handler = dependencies.auth_interaction_handler;
  if (dependencies.protocol_options_configurator) {
    dependencies.protocol_options_configurator(&options);
  }
  return options;
}

} // namespace

NativeHandshakeJob::NativeHandshakeJob(
    VpnEngineConfig config, NativeVpnEngineDependencies dependencies)
    : config_(std::move(config)), dependencies_(std::move(dependencies)) {}

ValidationResult NativeHandshakeJob::run(std::stop_token stop,
                                         NativeHandshakeResult *out) {
  if (out) {
    *out = {};
  }
  if (stop.stop_requested()) {
    return cancelled();
  }

  ValidationResult validation = validate_native_config(config_);
  if (!validation.ok) {
    return validation;
  }

  protocol::ParsedVpnUrl parsed;
  validation = protocol::parse_vpn_url(config_.server, &parsed);
  if (!validation.ok) {
    return validation;
  }

  std::unique_ptr<protocol::ProtocolTransport> transport;
  try {
    if (dependencies_.transport_factory) {
      transport = dependencies_.transport_factory();
    }
  } catch (const std::exception &e) {
    return invalid("native_transport_factory_failed", e.what());
  } catch (...) {
    return invalid("native_transport_factory_failed",
                   "Native transport factory failed.");
  }

  if (!transport) {
    return native_transport_unimplemented();
  }

  auto options = make_protocol_options(config_, parsed, dependencies_);
  auto protocol_session =
      std::make_unique<protocol::ProtocolSession>(options, transport.get());

  emit_event("native.starting", "info", "native VPN engine starting",
             {{"host", parsed.host}, {"port", std::to_string(parsed.port)}});
  emit_event("auth.started", "info", "password auth started");

  if (stop.stop_requested()) {
    transport->disconnect();
    return cancelled();
  }

  ValidationResult auth = protocol_session->authenticate();
  if (!auth.ok) {
    emit_event("auth.failed", "error", auth.message, {{"code", auth.code}});
    return auth;
  }
  emit_event("auth.succeeded", "info", "password auth succeeded");

  if (stop.stop_requested()) {
    transport->disconnect();
    return cancelled();
  }

  TunnelMetadata metadata;
  ValidationResult cstp = protocol_session->connect_cstp(&metadata);
  if (!cstp.ok) {
    emit_event("cstp.failed", "error", cstp.message, {{"code", cstp.code}});
    transport->disconnect();
    return cstp;
  }

  emit_event("cstp.connected", "info", "CSTP connect succeeded",
             {{"interface", metadata.interface_name},
              {"internal_ip", metadata.internal_ip4_address}});

  if (out) {
    out->metadata = std::move(metadata);
    out->transport = std::move(transport);
    out->session = std::move(protocol_session);
  }
  return {};
}

void NativeHandshakeJob::emit_event(
    std::string type, std::string level, std::string message,
    std::map<std::string, std::string> fields) {
  if (!dependencies_.event_sink) {
    return;
  }
  VpnEngineEvent event;
  event.type = std::move(type);
  event.level = std::move(level);
  event.message = std::move(message);
  event.fields = std::move(fields);
  dependencies_.event_sink->emit(event);
}

} // namespace vpn_engine
} // namespace ecnuvpn
