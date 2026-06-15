#include "packet_device_factory.hpp"

namespace exv::platform {

class PacketDevice {
public:
    virtual ~PacketDevice() = default;
    virtual bool is_open() const = 0;
};

class UnavailablePacketDeviceFactory : public PacketDeviceFactory {
public:
    std::unique_ptr<PacketDevice> create(const TunnelDeviceDescriptor& desc) override {
        (void)desc;
        return nullptr;
    }
};

std::unique_ptr<PacketDeviceFactory> PacketDeviceFactory::create() {
    return std::make_unique<UnavailablePacketDeviceFactory>();
}

} // namespace exv::platform
