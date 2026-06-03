#pragma once
#include <memory>
#include <string>
#include "tunnel_device_descriptor.hpp"

namespace exv::platform {

// Forward declaration
class PacketDevice;

class PacketDeviceFactory {
public:
    virtual ~PacketDeviceFactory() = default;

    // Create a packet device for the given descriptor
    virtual std::unique_ptr<PacketDevice> create(const TunnelDeviceDescriptor& desc) = 0;

    // Platform factory
    static std::unique_ptr<PacketDeviceFactory> create();
};

} // namespace exv::platform
