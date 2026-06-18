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
    cfg.extra_args = {"--csd-wrapper", "C:\\secret\\wrapper.cmd"};
    auto result = exv::core::validate_native_app_config(cfg);
    ok = expect(!result.ok && result.code == "unsupported_extra_args",
                "reserved CSD wrapper arg should be rejected before engine start") &&
         ok;
    ok = expect(result.message.find("--csd-wrapper") != std::string::npos,
                "unsupported extra_args message should identify rejected arg") &&
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
                "native mapper should force CSTP/TLS until production DTLS exists") &&
         ok;
  }

  {
    ecnuvpn::Config cfg = app_config();
    cfg.extra_args = {"--cookie=SECRET_COOKIE_SEED"};
    auto result = exv::core::validate_native_app_config(cfg);
    ok = expect(!result.ok && result.code == "unsupported_extra_args",
                "unsupported native extra_args should be rejected") &&
         ok;
    ok = expect(result.message.find("SECRET_COOKIE_SEED") == std::string::npos,
                "unsupported extra_args error must not expose argument value") &&
         ok;
    ok = expect(result.message.find("--cookie") != std::string::npos,
                "unsupported extra_args error should name unsupported flag") &&
         ok;
  }

  {
    ecnuvpn::Config cfg = app_config();
    cfg.extra_args = {
        "--no-dtls",
        "--useragent=ECNU-VPN custom native UA",
        "--authgroup=students",
        "--csd-wrapper=C:/Tools/csd-wrapper.bat",
    };
    ecnuvpn::vpn_engine::VpnEngineConfig engine_cfg;
    auto result =
        exv::core::make_native_engine_config(cfg, kPassword, &engine_cfg);
    ok = expect(result.ok,
                "whitelisted native extra_args should map successfully") &&
         ok;
    ok = expect(engine_cfg.disable_dtls,
                "--no-dtls should set disable_dtls=true") &&
         ok;
    ok = expect(engine_cfg.useragent == "ECNU-VPN custom native UA",
                "--useragent should override useragent") &&
         ok;
    ok = expect(engine_cfg.auth_group == "students",
                "--authgroup should map to auth_group") &&
         ok;
    ok = expect(engine_cfg.csd_wrapper == "C:/Tools/csd-wrapper.bat",
                "--csd-wrapper should be retained for controlled future CSD handling") &&
         ok;
  }

  {
    ecnuvpn::Config cfg = app_config();
    cfg.disable_dtls = true;
    ecnuvpn::vpn_engine::VpnEngineConfig engine_cfg;
    auto result =
        exv::core::make_native_engine_config(cfg, kPassword, &engine_cfg);
    ok = expect(result.ok, "valid disabled-DTLS config should map") && ok;
    ok = expect(engine_cfg.disable_dtls,
                "disable_dtls=true should map to engine config") &&
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
