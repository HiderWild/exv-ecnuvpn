#include "vpn_engine/native_handshake_job.hpp"

#include "vpn_engine/packet_device.hpp"
#include "vpn_engine/protocol/session.hpp"

#include <iostream>
#include <memory>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << "\n";
  return false;
}

class FakeTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
  explicit FakeTransport(int *auth_count, int *cstp_count,
                         int *disconnect_count)
      : auth_count_(auth_count), cstp_count_(cstp_count),
        disconnect_count_(disconnect_count) {}

  ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
      const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions
          & /*options*/) override {
    ++*auth_count_;
    ecnuvpn::vpn_engine::protocol::AuthResult result;
    result.ok = true;
    result.cookie = "webvpn=HANDSHAKE";
    return result;
  }

  ecnuvpn::vpn_engine::ValidationResult
  connect_cstp(const std::string &cookie,
               ecnuvpn::vpn_engine::TunnelMetadata *metadata) override {
    ++*cstp_count_;
    last_cookie = cookie;
    if (!metadata) {
      return {false, "null_metadata", "metadata output is required"};
    }
    metadata->interface_name = "cstp0";
    metadata->internal_ip4_address = "10.0.0.42";
    metadata->internal_ip4_netmask = "255.255.255.0";
    metadata->mtu = 1400;
    metadata->routes = {"10.1.0.0/16"};
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_control(ecnuvpn::vpn_engine::protocol::InboundFrameKind /*kind*/)
      override {
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  receive_frame(ecnuvpn::vpn_engine::protocol::InboundFrame * /*out*/)
      override {
    return {false, "not_used", "not used"};
  }

  void disconnect() override { ++*disconnect_count_; }
  void reset_for_reconnect() override {}

  std::string last_cookie;

private:
  int *auth_count_;
  int *cstp_count_;
  int *disconnect_count_;
};

class FailingTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
  explicit FailingTransport(int *disconnect_count)
      : disconnect_count_(disconnect_count) {}

  ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
      const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions
          & /*options*/) override {
    ecnuvpn::vpn_engine::protocol::AuthResult result;
    result.ok = true;
    result.cookie = "webvpn=HANDSHAKE";
    return result;
  }

  ecnuvpn::vpn_engine::ValidationResult
  connect_cstp(const std::string & /*cookie*/,
               ecnuvpn::vpn_engine::TunnelMetadata * /*metadata*/) override {
    return {false, "cstp_failed", "CSTP failed"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_control(ecnuvpn::vpn_engine::protocol::InboundFrameKind /*kind*/)
      override {
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  receive_frame(ecnuvpn::vpn_engine::protocol::InboundFrame * /*out*/)
      override {
    return {false, "not_used", "not used"};
  }

  void disconnect() override { ++*disconnect_count_; }
  void reset_for_reconnect() override {}

private:
  int *disconnect_count_;
};

ecnuvpn::vpn_engine::VpnEngineConfig valid_config() {
  ecnuvpn::vpn_engine::VpnEngineConfig cfg;
  cfg.server = "https://vpn.example.test";
  cfg.username = "alice";
  cfg.password = "secret";
  cfg.useragent = "test";
  cfg.disable_dtls = true;
  return cfg;
}

} // namespace

int main() {
  namespace ve = ecnuvpn::vpn_engine;

  bool ok = true;

  {
    int auth_count = 0;
    int cstp_count = 0;
    int disconnect_count = 0;
    int packet_device_count = 0;
    int network_config_count = 0;
    FakeTransport *transport_ptr = nullptr;

    ve::NativeVpnEngineDependencies deps;
    deps.transport_factory = [&] {
      auto transport = std::make_unique<FakeTransport>(
          &auth_count, &cstp_count, &disconnect_count);
      transport_ptr = transport.get();
      return transport;
    };
    deps.packet_device_factory = [&] {
      ++packet_device_count;
      return nullptr;
    };
    deps.network_configurator = [&](const ve::TunnelMetadata &,
                                    ve::DeviceConfig *) {
      ++network_config_count;
      return ve::ValidationResult{};
    };

    ve::NativeHandshakeJob job(valid_config(), std::move(deps));
    ve::NativeHandshakeResult result;
    auto validation = job.run(std::stop_token{}, &result);

    ok = expect(validation.ok, "handshake succeeds") && ok;
    ok = expect(auth_count == 1, "auth is called once") && ok;
    ok = expect(cstp_count == 1, "cstp is called once") && ok;
    ok = expect(disconnect_count == 0, "successful handshake stays connected") &&
         ok;
    ok = expect(packet_device_count == 0,
                "handshake must not create a packet device") &&
         ok;
    ok = expect(network_config_count == 0,
                "handshake must not apply network config") &&
         ok;
    ok = expect(result.metadata.internal_ip4_address == "10.0.0.42",
                "metadata is returned") &&
         ok;
    ok = expect(result.transport.get() == transport_ptr,
                "connected transport ownership is returned") &&
         ok;
    ok = expect(result.session != nullptr, "protocol session is returned") &&
         ok;
  }

  {
    int disconnect_count = 0;
    ve::NativeVpnEngineDependencies deps;
    deps.transport_factory = [&] {
      return std::make_unique<FailingTransport>(&disconnect_count);
    };

    ve::NativeHandshakeJob job(valid_config(), std::move(deps));
    ve::NativeHandshakeResult result;
    auto validation = job.run(std::stop_token{}, &result);

    ok = expect(!validation.ok, "cstp failure is returned") && ok;
    ok = expect(validation.code == "cstp_failed",
                "cstp failure code is preserved") &&
         ok;
    ok = expect(disconnect_count == 1,
                "failed cstp handshake disconnects transport") &&
         ok;
    ok = expect(result.transport == nullptr,
                "failed handshake does not return transport") &&
         ok;
    ok = expect(result.session == nullptr,
                "failed handshake does not return session") &&
         ok;
  }

  if (ok) {
    std::cout << "native_handshake_job_test: all assertions passed\n";
  }
  return ok ? 0 : 1;
}
