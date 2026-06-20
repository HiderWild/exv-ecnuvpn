#include "vpn_engine/native_engine.hpp"

#include <memory>

namespace exv {
namespace vpn_engine {

namespace {

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

class UnsupportedProtocolTransport final
    : public protocol::ProtocolTransport {
public:
  protocol::AuthResult
  authenticate(const protocol::ProtocolSessionOptions & /*options*/) override {
    protocol::AuthResult result;
    result.ok = false;
    result.error_code = "native_transport_unavailable";
    result.error_message =
        "Native engine production TLS transport is not available on this platform.";
    return result;
  }

  ValidationResult connect_cstp(const std::string & /*cookie*/,
                                TunnelMetadata * /*metadata*/) override {
    return invalid("native_transport_unavailable",
                   "Native engine production TLS transport is not available on this platform.");
  }

  ValidationResult
  send_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return invalid("native_transport_unavailable",
                   "Native engine production TLS transport is not available on this platform.");
  }

  ValidationResult
  send_control(protocol::InboundFrameKind /*kind*/) override {
    return invalid("native_transport_unavailable",
                   "Native engine production TLS transport is not available on this platform.");
  }

  ValidationResult receive_frame(protocol::InboundFrame * /*out*/) override {
    return invalid("native_transport_unavailable",
                   "Native engine production TLS transport is not available on this platform.");
  }

  void disconnect() override {}
  void reset_for_reconnect() override {}
};

class UnsupportedPacketDevice final : public PacketDevice {
public:
  ValidationResult open(const DeviceConfig & /*config*/) override {
    return invalid("native_packet_device_unavailable",
                   "Native engine platform packet device is not available on this platform.");
  }

  ValidationResult open(const TunnelMetadata & /*metadata*/) override {
    return invalid("native_packet_device_unavailable",
                   "Native engine platform packet device is not available on this platform.");
  }

  ValidationResult read_packet(std::vector<std::uint8_t> * /*packet*/) override {
    return invalid("native_packet_device_unavailable",
                   "Native engine platform packet device is not available on this platform.");
  }

  ValidationResult
  write_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return invalid("native_packet_device_unavailable",
                   "Native engine platform packet device is not available on this platform.");
  }

  void close() override {}
};

} // namespace

NativeVpnEngineDependencies default_native_engine_dependencies() {
  NativeVpnEngineDependencies deps;

  deps.transport_factory = [] {
    return std::unique_ptr<protocol::ProtocolTransport>(
        new UnsupportedProtocolTransport());
  };
  deps.packet_device_factory = [] {
    return std::unique_ptr<PacketDevice>(new UnsupportedPacketDevice());
  };

  return deps;
}

} // namespace vpn_engine
} // namespace exv
