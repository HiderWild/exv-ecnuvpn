#include "packet_device_factory.hpp"
#include <iostream>

namespace exv::platform {

// Minimal platform-level PacketDevice for the stub.
// Real implementations will live in platform-specific directories.
class PacketDevice {
public:
    virtual ~PacketDevice() = default;
    virtual bool is_open() const = 0;
};

class StubPacketDevice : public PacketDevice {
public:
    bool is_open() const override { return true; }
};

class StubPacketDeviceFactory : public PacketDeviceFactory {
public:
    std::unique_ptr<PacketDevice> create(const TunnelDeviceDescriptor& desc) override {
        std::cout << "[StubPacketDeviceFactory] create device for: " << desc.path << std::endl;
        return std::make_unique<StubPacketDevice>();
    }
};

std::unique_ptr<PacketDeviceFactory> PacketDeviceFactory::create() {
    return std::make_unique<StubPacketDeviceFactory>();
}

} // namespace exv::platform
