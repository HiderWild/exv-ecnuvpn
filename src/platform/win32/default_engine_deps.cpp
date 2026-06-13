#include "vpn_engine/native_engine.hpp"
#include "vpn_engine/protocol/production_transport.hpp"
#include "platform/win32/native_packet_device.hpp"
#include "platform/win32/native_tls_stream.hpp"

#include <memory>

namespace ecnuvpn {
namespace vpn_engine {

NativeVpnEngineDependencies default_native_engine_dependencies() {
  NativeVpnEngineDependencies deps;

  deps.transport_factory = [] {
    std::unique_ptr<protocol::TlsStream> stream(new platform::NativeTlsStream());
    return std::unique_ptr<protocol::ProtocolTransport>(
        new protocol::ProductionProtocolTransport(std::move(stream)));
  };
  deps.packet_device_factory = [] {
    return platform::create_native_packet_device();
  };

  return deps;
}

} // namespace vpn_engine
} // namespace ecnuvpn
