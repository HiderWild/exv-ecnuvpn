#pragma once

#include "vpn_engine/engine.hpp"

#include <cstdint>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {

// Forward declaration to keep this interface platform-neutral.
// A full definition can live in a higher-level session/tunnel metadata header.
struct TunnelMetadata;

class PacketDevice {
public:
  virtual ~PacketDevice() = default;

  virtual ValidationResult open(const TunnelMetadata &metadata) = 0;
  virtual ValidationResult read_packet(std::vector<std::uint8_t> *packet) = 0;
  virtual ValidationResult write_packet(const std::vector<std::uint8_t> &packet) = 0;
  virtual void close() = 0;
};

} // namespace vpn_engine
} // namespace ecnuvpn
