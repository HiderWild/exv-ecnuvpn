#pragma once

#include "platform/win32/native_ip_config.hpp"
#include "platform/win32/native_wintun.hpp"
#include "vpn_engine/packet_device.hpp"

#include <cstdint>
#include <functional>
#include <memory>
#include <vector>

namespace ecnuvpn {
namespace platform {

class NativePacketDeviceWintunSession {
public:
  virtual ~NativePacketDeviceWintunSession() = default;

  virtual NativeWintunStartResult start() = 0;
  virtual vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) = 0;
  virtual vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) = 0;
  virtual void stop() = 0;
};

class NativePacketDeviceIpConfig {
public:
  virtual ~NativePacketDeviceIpConfig() = default;

  virtual NativeIpConfigResult
  configure(const vpn_engine::TunnelMetadata &metadata) = 0;
  virtual NativeIpConfigResult cleanup() = 0;
};

struct NativePacketDeviceDependencies {
  std::function<std::unique_ptr<NativePacketDeviceWintunSession>()>
      create_wintun_session;
  std::function<std::unique_ptr<NativePacketDeviceIpConfig>(std::uint32_t)>
      create_ip_config;
};

NativePacketDeviceDependencies default_native_packet_device_dependencies();

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
  std::unique_ptr<NativePacketDeviceWintunSession> wintun_session_;
  std::unique_ptr<NativePacketDeviceIpConfig> ip_config_;
  bool open_ = false;
};

std::unique_ptr<vpn_engine::PacketDevice> create_native_packet_device();

} // namespace platform
} // namespace ecnuvpn
