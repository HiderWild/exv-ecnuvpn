#pragma once

#include "vpn_engine/engine.hpp"

#include <cstdint>
#include <string>
#include <vector>

namespace exv {
namespace vpn_engine {

// Forward declaration to keep this interface platform-neutral.
// A full definition can live in a higher-level session/tunnel metadata header.
struct TunnelMetadata;

/// Device-only configuration for PacketDevice::open().
/// Network routes and DNS should be applied separately via
/// PlatformNetworkOps, NOT passed to open().  This struct replaces
/// the previous practice of passing full TunnelMetadata (which
/// included routes) into open().
struct DeviceConfig {
  std::string interface_name;   // Platform-specific adapter name
  int mtu = 1290;
};

class PacketDevice {
public:
  virtual ~PacketDevice() = default;

  /// Open the tunnel device with device-only configuration.
  /// Network routes and DNS are applied separately via PlatformNetworkOps.
  virtual ValidationResult open(const DeviceConfig &config) = 0;

  /// DEPRECATED: Opens the tunnel device with full TunnelMetadata including
  /// network routes.  New code should use open(DeviceConfig) and apply
  /// routes/DNS via PlatformNetworkOps::apply_tunnel_config().
  virtual ValidationResult open(const TunnelMetadata &metadata) = 0;

  virtual ValidationResult read_packet(std::vector<std::uint8_t> *packet) = 0;
  virtual ValidationResult write_packet(const std::vector<std::uint8_t> &packet) = 0;
  virtual void close() = 0;
};

} // namespace vpn_engine
} // namespace exv
