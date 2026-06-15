#include "core/tunnel_controller/native_engine_config_mapper.hpp"

#include <utility>

namespace exv::core {
namespace {

ecnuvpn::vpn_engine::ValidationResult invalid(std::string code,
                                              std::string message) {
  ecnuvpn::vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

} // namespace

ecnuvpn::vpn_engine::ValidationResult
validate_native_app_config(const ecnuvpn::Config &cfg) {
  if (!cfg.extra_args.empty()) {
    return invalid(
        "unsupported_extra_args",
        "Native VPN engine does not support legacy OpenConnect extra_args.");
  }
  if (cfg.server.empty()) {
    return invalid("config_invalid", "VPN server is not configured.");
  }
  if (cfg.username.empty()) {
    return invalid("config_invalid", "VPN username is not configured.");
  }
  return ecnuvpn::vpn_engine::ValidationResult{};
}

ecnuvpn::vpn_engine::ValidationResult make_native_engine_config(
    const ecnuvpn::Config &cfg, const std::string &plaintext_password,
    ecnuvpn::vpn_engine::VpnEngineConfig *out) {
  if (!out) {
    return invalid("invalid_output",
                   "Native engine config output pointer is null.");
  }

  auto validation = validate_native_app_config(cfg);
  if (!validation.ok) {
    return validation;
  }

  ecnuvpn::vpn_engine::VpnEngineConfig engine_cfg;
  engine_cfg.engine = cfg.vpn_engine;
  engine_cfg.server = cfg.server;
  engine_cfg.username = cfg.username;
  engine_cfg.password = plaintext_password;
  engine_cfg.useragent = cfg.useragent;
  engine_cfg.mtu = cfg.mtu;
  engine_cfg.routes = cfg.routes;
  engine_cfg.windows_tunnel_driver = cfg.windows_tunnel_driver;
  engine_cfg.windows_tap_interface = cfg.windows_tap_interface;
  engine_cfg.auto_reconnect = cfg.auto_reconnect;
  // Native engine is CSTP/TLS-only. The user-facing disable_dtls flag belongs
  // to the legacy OpenConnect command-line path, so core forces CSTP here.
  engine_cfg.disable_dtls = true;

  *out = std::move(engine_cfg);
  return ecnuvpn::vpn_engine::ValidationResult{};
}

} // namespace exv::core
