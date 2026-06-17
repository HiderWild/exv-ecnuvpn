#include "core/tunnel_controller/native_engine_config_mapper.hpp"

#include <iostream>
#include <string>

namespace {

constexpr const char *kPassword = "test-mock";

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

ecnuvpn::Config app_config() {
  ecnuvpn::Config cfg;
  cfg.vpn_engine = "native";
  cfg.server = "https://vpn.example.edu";
  cfg.username = "alice";
  cfg.useragent = "ECNU-VPN mapper test";
  cfg.mtu = 1400;
  cfg.routes = {"10.0.0.0/8", "172.16.0.0/12"};
  cfg.windows_tunnel_driver = "wintun";
  cfg.windows_tap_interface = "ECNU TAP";
  cfg.auto_reconnect = false;
  cfg.disable_dtls = false;
  return cfg;
}

} // namespace

int main() {
  bool ok = true;

  {
    ecnuvpn::Config cfg = app_config();
    cfg.server.clear();
    auto result = exv::core::validate_native_app_config(cfg);
    ok = expect(!result.ok && result.code == "config_invalid",
                "empty server should be config_invalid") &&
         ok;
  }

  {
    ecnuvpn::Config cfg = app_config();
    cfg.username.clear();
    auto result = exv::core::validate_native_app_config(cfg);
    ok = expect(!result.ok && result.code == "config_invalid",
                "empty username should be config_invalid") &&
         ok;
  }

  {
    ecnuvpn::Config cfg = app_config();
    cfg.extra_args = {"--dump-http-traffic"};
    auto result = exv::core::validate_native_app_config(cfg);
    ok = expect(!result.ok && result.code == "unsupported_extra_args",
                "legacy extra_args should be rejected before engine start") &&
         ok;
  }

  {
    ecnuvpn::Config cfg = app_config();
    ecnuvpn::vpn_engine::VpnEngineConfig engine_cfg;
    auto result =
        exv::core::make_native_engine_config(cfg, kPassword, &engine_cfg);
    ok = expect(result.ok, "valid app config should map to engine config") &&
         ok;
    ok = expect(engine_cfg.engine == cfg.vpn_engine,
                "engine field should map") &&
         ok;
    ok = expect(engine_cfg.server == cfg.server,
                "server should map") &&
         ok;
    ok = expect(engine_cfg.username == cfg.username,
                "username should map") &&
         ok;
    ok = expect(engine_cfg.password == kPassword,
                "plaintext password should be copied into engine config") &&
         ok;
    ok = expect(engine_cfg.useragent == cfg.useragent,
                "useragent should map") &&
         ok;
    ok = expect(engine_cfg.mtu == cfg.mtu, "mtu should map") && ok;
    ok = expect(engine_cfg.routes == cfg.routes, "routes should map") && ok;
    ok = expect(engine_cfg.windows_tunnel_driver == cfg.windows_tunnel_driver,
                "windows_tunnel_driver should map") &&
         ok;
    ok = expect(engine_cfg.windows_tap_interface == cfg.windows_tap_interface,
                "windows_tap_interface should map") &&
         ok;
    ok = expect(engine_cfg.auto_reconnect == cfg.auto_reconnect,
                "auto_reconnect should map") &&
         ok;
    ok = expect(engine_cfg.disable_dtls,
                "native CSTP mode should force disable_dtls=true") &&
         ok;
  }

  {
    ecnuvpn::Config cfg = app_config();
    auto result = exv::core::make_native_engine_config(cfg, kPassword, nullptr);
    ok = expect(!result.ok && result.code == "invalid_output",
                "null output pointer should be rejected") &&
         ok;
  }

  if (ok) {
    std::cout << "native_engine_config_mapper_test: all assertions passed\n";
  } else {
    std::cerr << "native_engine_config_mapper_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
