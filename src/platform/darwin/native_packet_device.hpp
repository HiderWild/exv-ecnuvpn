#pragma once

#include "platform/darwin/native_route_config.hpp"
#include "platform/darwin/native_utun.hpp"
#include "vpn_engine/packet_device.hpp"

#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <vector>

namespace ecnuvpn {
namespace platform {

struct NativeDarwinPacketIoApi {
  std::function<int(int)> set_nonblocking_fd;
  std::function<int(int, int)> wait_readable_fd;
  std::function<int(int, int)> wait_writable_fd;
  std::function<std::ptrdiff_t(int, void *, std::size_t)> read_fd;
  std::function<std::ptrdiff_t(int, const void *, std::size_t)> write_fd;
  std::function<int()> last_error;
};

class NativePacketDeviceUtunSession {
public:
  virtual ~NativePacketDeviceUtunSession() = default;

  virtual NativeUtunStartResult start() = 0;
  virtual vpn_engine::ValidationResult
  read_frame(std::vector<std::uint8_t> *frame) = 0;
  virtual vpn_engine::ValidationResult
  write_frame(const std::vector<std::uint8_t> &frame) = 0;
  virtual void stop() = 0;
};

class NativePacketDeviceRouteConfig {
public:
  virtual ~NativePacketDeviceRouteConfig() = default;

  virtual NativeDarwinRouteConfigResult
  configure(const vpn_engine::TunnelMetadata &metadata) = 0;
  virtual NativeDarwinRouteConfigResult cleanup() = 0;
};

struct NativePacketDeviceDependencies {
  std::function<std::unique_ptr<NativePacketDeviceUtunSession>(
      const vpn_engine::TunnelMetadata &)>
      create_utun_session;
  std::function<std::unique_ptr<NativePacketDeviceRouteConfig>(
      const NativeUtunMetadata &)>
      create_route_config;
};

NativePacketDeviceDependencies default_native_packet_device_dependencies();
NativeDarwinPacketIoApi default_native_darwin_packet_io_api();

std::unique_ptr<NativePacketDeviceUtunSession>
create_native_darwin_utun_packet_session(
    const vpn_engine::TunnelMetadata &metadata);

std::unique_ptr<NativePacketDeviceUtunSession>
create_native_darwin_utun_packet_session(
    const vpn_engine::TunnelMetadata &metadata, NativeUtunApi utun_api,
    NativeDarwinPacketIoApi io_api);

class NativePacketDevice final : public vpn_engine::PacketDevice {
public:
  NativePacketDevice();
  explicit NativePacketDevice(NativePacketDeviceDependencies dependencies);
  ~NativePacketDevice() override;

  NativePacketDevice(const NativePacketDevice &) = delete;
  NativePacketDevice &operator=(const NativePacketDevice &) = delete;

  vpn_engine::ValidationResult
  open(const vpn_engine::DeviceConfig &config) override;
  vpn_engine::ValidationResult
  open(const vpn_engine::TunnelMetadata &metadata) override;
  vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override;
  vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) override;
  void close() override;

private:
  vpn_engine::ValidationResult close_resources();

  NativePacketDeviceDependencies dependencies_;
  std::unique_ptr<NativePacketDeviceUtunSession> utun_session_;
  std::unique_ptr<NativePacketDeviceRouteConfig> route_config_;
  bool open_ = false;
};

std::unique_ptr<vpn_engine::PacketDevice> create_native_packet_device();

} // namespace platform
} // namespace ecnuvpn
