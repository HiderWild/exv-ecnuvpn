#include "platform/darwin/native_packet_device.hpp"

#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <string>
#include <utility>
#include <vector>

#if defined(__APPLE__)
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>
#endif

namespace exv {
namespace platform {
namespace {

constexpr std::size_t kUtunAddressFamilyHeaderSize = 4;
constexpr std::size_t kMaximumIpPacketSize = 65535;
constexpr std::size_t kMaximumUtunFrameSize =
    kUtunAddressFamilyHeaderSize + kMaximumIpPacketSize;
constexpr int kDefaultMtu = 1290;
constexpr int kUtunIoPollTimeoutMs = 100;
constexpr int kMaximumTransientWriteRetries = 16;
// Begin inlined from platform/darwin/native_packet_device_helpers include-unit
vpn_engine::ValidationResult invalid(std::string code, std::string message) {
  vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

int unsupported_native_api_error() { return ENOSYS; }

int errno_or_io_error() { return errno == 0 ? EIO : errno; }

int address_family_inet() {
#if defined(AF_INET)
  return AF_INET;
#else
  return 2;
#endif
}

int address_family_inet6() {
#if defined(AF_INET6)
  return AF_INET6;
#else
  return 30;
#endif
}

int last_error(const NativeDarwinPacketIoApi &api) {
  if (!api.last_error)
    return 0;
  return api.last_error();
}

bool has_required_io_api(const NativeDarwinPacketIoApi &api) {
  return static_cast<bool>(api.set_nonblocking_fd) &&
         static_cast<bool>(api.wait_readable_fd) &&
         static_cast<bool>(api.wait_writable_fd) &&
         static_cast<bool>(api.read_fd) && static_cast<bool>(api.write_fd);
}

bool is_interrupted(int system_error) { return system_error == EINTR; }

bool is_would_block(int system_error) {
  return system_error == EAGAIN || system_error == EWOULDBLOCK;
}

vpn_engine::ValidationResult retryable(std::string code,
                                       std::string message) {
  return invalid(std::move(code), std::move(message));
}

NativeUtunStartResult packet_io_start_failure(NativeUtunError error,
                                              std::string detail,
                                              int system_error = 0) {
  NativeUtunStartResult result;
  result.error = error;
  result.detail = std::move(detail);
  result.system_error = system_error;
  return result;
}

std::uint32_t read_address_family(const std::vector<std::uint8_t> &frame) {
  return (static_cast<std::uint32_t>(frame[0]) << 24) |
         (static_cast<std::uint32_t>(frame[1]) << 16) |
         (static_cast<std::uint32_t>(frame[2]) << 8) |
         static_cast<std::uint32_t>(frame[3]);
}

void append_address_family(std::vector<std::uint8_t> *frame,
                           std::uint32_t family) {
  frame->push_back(static_cast<std::uint8_t>((family >> 24) & 0xff));
  frame->push_back(static_cast<std::uint8_t>((family >> 16) & 0xff));
  frame->push_back(static_cast<std::uint8_t>((family >> 8) & 0xff));
  frame->push_back(static_cast<std::uint8_t>(family & 0xff));
}

vpn_engine::ValidationResult
address_family_for_packet(const std::vector<std::uint8_t> &packet,
                          std::uint32_t *family) {
  if (!family)
    return invalid("packet_device_invalid_argument",
                   "address family output pointer is null");
  if (packet.empty())
    return invalid("packet_device_invalid_packet", "packet is empty");
  if (packet.size() > kMaximumIpPacketSize)
    return invalid("packet_device_invalid_packet", "packet is too large");

  const std::uint8_t version = static_cast<std::uint8_t>(packet[0] >> 4);
  if (version == 4) {
    *family = static_cast<std::uint32_t>(address_family_inet());
    return {};
  }
  if (version == 6) {
    *family = static_cast<std::uint32_t>(address_family_inet6());
    return {};
  }
  return invalid("darwin_utun_unsupported_ip_version",
                 "packet IP version is not IPv4 or IPv6");
}

vpn_engine::ValidationResult
validate_read_frame(const std::vector<std::uint8_t> &frame,
                    std::uint32_t *family) {
  if (frame.size() < kUtunAddressFamilyHeaderSize)
    return invalid("darwin_utun_invalid_frame",
                   "utun frame is shorter than address-family header");
  if (frame.size() == kUtunAddressFamilyHeaderSize)
    return invalid("darwin_utun_invalid_frame",
                   "utun frame does not contain an IP packet");

  const std::uint32_t parsed_family = read_address_family(frame);
  if (parsed_family != static_cast<std::uint32_t>(address_family_inet()) &&
      parsed_family != static_cast<std::uint32_t>(address_family_inet6())) {
    return invalid("darwin_utun_unsupported_address_family",
                   "utun frame address family is not AF_INET or AF_INET6");
  }

  if (family)
    *family = parsed_family;
  return {};
}

NativeUtunConfig utun_config_from_metadata(
    const vpn_engine::TunnelMetadata &metadata) {
  NativeUtunConfig config;
  config.mtu = metadata.mtu;
  return config;
}

NativeDarwinRouteConfigOptions
route_options_from_utun(const NativeUtunMetadata &metadata) {
  NativeDarwinRouteConfigOptions options;
  options.interface_name = metadata.interface_name;
  options.configured_mtu =
      metadata.mtu > 0 ? metadata.mtu : static_cast<int>(kDefaultMtu);
  return options;
}
// End inlined from platform/darwin/native_packet_device_helpers include-unit
// Begin inlined from platform/darwin/native_packet_device_sessions include-unit
class RealUtunPacketSession final : public NativePacketDeviceUtunSession {
public:
  explicit RealUtunPacketSession(const vpn_engine::TunnelMetadata &metadata)
      : RealUtunPacketSession(metadata, default_native_utun_api(),
                              default_native_darwin_packet_io_api()) {}

  RealUtunPacketSession(const vpn_engine::TunnelMetadata &metadata,
                        NativeUtunApi utun_api,
                        NativeDarwinPacketIoApi io)
      : utun_(std::move(utun_api), utun_config_from_metadata(metadata)),
        io_(std::move(io)) {}

  ~RealUtunPacketSession() override { stop(); }

  NativeUtunStartResult start() override {
    if (!has_required_io_api(io_)) {
      return packet_io_start_failure(
          NativeUtunError::api_missing,
          "native Darwin packet I/O API table is incomplete");
    }

    NativeUtunStartResult result = utun_.start();
    if (!result.ok())
      return result;

    const int error = io_.set_nonblocking_fd(result.metadata.fd);
    if (error != 0) {
      utun_.stop();
      return packet_io_start_failure(
          NativeUtunError::socket_open_failed,
          "failed to set utun fd nonblocking", error);
    }

    return result;
  }

  vpn_engine::ValidationResult
  read_frame(std::vector<std::uint8_t> *frame) override {
    if (!frame)
      return invalid("packet_device_invalid_argument",
                     "frame output pointer is null");
    if (!utun_.running() || utun_.metadata().fd < 0)
      return invalid("packet_device_closed", "packet device is closed");
    if (!has_required_io_api(io_))
      return invalid("packet_device_api_missing",
                     "native Darwin packet I/O API table is incomplete");

    const int fd = utun_.metadata().fd;
    const int ready = io_.wait_readable_fd(fd, kUtunIoPollTimeoutMs);
    if (ready == 0)
      return retryable("no_data", "no utun frame is available");
    if (ready < 0) {
      const int system_error = last_error(io_);
      if (is_interrupted(system_error))
        return retryable("try_again", "utun read wait was interrupted");
      if (is_would_block(system_error))
        return retryable("would_block", "utun read would block");
      return invalid("darwin_utun_read_failed",
                     "failed to wait for utun read with errno " +
                         std::to_string(system_error));
    }

    std::vector<std::uint8_t> buffer(kMaximumUtunFrameSize);
    const std::ptrdiff_t read_count =
        io_.read_fd(fd, buffer.data(), buffer.size());
    if (read_count < 0) {
      const int system_error = last_error(io_);
      if (is_interrupted(system_error))
        return retryable("try_again", "utun read was interrupted");
      if (is_would_block(system_error))
        return retryable("would_block", "utun read would block");
      return invalid("darwin_utun_read_failed",
                     "failed to read utun frame with errno " +
                         std::to_string(system_error));
    }
    if (read_count == 0)
      return invalid("darwin_utun_read_failed", "utun fd reached end of file");

    frame->assign(buffer.begin(), buffer.begin() + read_count);
    return {};
  }

  vpn_engine::ValidationResult
  write_frame(const std::vector<std::uint8_t> &frame) override {
    if (!utun_.running() || utun_.metadata().fd < 0)
      return invalid("packet_device_closed", "packet device is closed");
    if (!has_required_io_api(io_))
      return invalid("packet_device_api_missing",
                     "native Darwin packet I/O API table is incomplete");
    if (frame.empty())
      return invalid("darwin_utun_invalid_frame", "utun frame is empty");

    const int fd = utun_.metadata().fd;
    std::size_t offset = 0;
    int transient_retries = 0;
    while (offset < frame.size()) {
      const int ready = io_.wait_writable_fd(fd, kUtunIoPollTimeoutMs);
      if (ready == 0)
        return retryable("would_block", "utun write would block");
      if (ready < 0) {
        const int system_error = last_error(io_);
        if (is_interrupted(system_error)) {
          if (++transient_retries > kMaximumTransientWriteRetries)
            return retryable("try_again", "utun write wait was interrupted");
          continue;
        }
        if (is_would_block(system_error))
          return retryable("would_block", "utun write would block");
        return invalid("darwin_utun_write_failed",
                       "failed to wait for utun write with errno " +
                           std::to_string(system_error));
      }

      const std::ptrdiff_t written = io_.write_fd(
          fd, frame.data() + offset, frame.size() - offset);
      if (written < 0) {
        const int system_error = last_error(io_);
        if (is_interrupted(system_error)) {
          if (++transient_retries > kMaximumTransientWriteRetries)
            return retryable("try_again", "utun write was interrupted");
          continue;
        }
        if (is_would_block(system_error)) {
          if (++transient_retries > kMaximumTransientWriteRetries)
            return retryable("would_block", "utun write would block");
          continue;
        }
        return invalid("darwin_utun_write_failed",
                       "failed to write utun frame with errno " +
                           std::to_string(system_error));
      }
      if (written == 0)
        return invalid("darwin_utun_write_failed",
                       "utun write made no progress");

      const std::size_t written_size = static_cast<std::size_t>(written);
      if (written_size > frame.size() - offset)
        return invalid("darwin_utun_write_failed",
                       "utun write reported too many bytes");

      offset += written_size;
      transient_retries = 0;
    }

    return {};
  }

  void stop() override { utun_.stop(); }

private:
  NativeUtun utun_;
  NativeDarwinPacketIoApi io_;
};

class RealRouteConfig final : public NativePacketDeviceRouteConfig {
public:
  explicit RealRouteConfig(const NativeUtunMetadata &metadata)
      : config_(default_native_darwin_route_api(),
                route_options_from_utun(metadata)) {}

  NativeDarwinRouteConfigResult
  configure(const vpn_engine::TunnelMetadata &metadata) override {
    return config_.configure(metadata);
  }

  NativeDarwinRouteConfigResult cleanup() override {
    return config_.cleanup();
  }

private:
  NativeDarwinRouteConfig config_;
};
// End inlined from platform/darwin/native_packet_device_sessions include-unit
// Begin inlined from platform/darwin/native_packet_device_errors include-unit
vpn_engine::ValidationResult
utun_start_failure_result(const NativeUtunStartResult &start) {
  return invalid(std::string("native_utun_") +
                     native_utun_error_code(start.error),
                 start.detail.empty() ? "failed to start native utun session"
                                      : start.detail);
}

vpn_engine::ValidationResult
route_config_failure_result(const NativeDarwinRouteConfigResult &config) {
  std::string message = config.message.empty()
                            ? "failed to configure native Darwin routes"
                            : config.message;
  if (!config.target.empty())
    message += ": " + config.target;
  return invalid(std::string("native_darwin_route_config_") +
                     native_darwin_route_config_error_code(config.error),
                 message);
}

vpn_engine::ValidationResult route_cleanup_failure_result(
    const NativeDarwinRouteConfigResult &cleanup) {
  std::string message = cleanup.message.empty()
                            ? "failed to cleanup native Darwin routes"
                            : cleanup.message;
  if (!cleanup.target.empty())
    message += ": " + cleanup.target;
  return invalid(std::string("native_darwin_route_config_") +
                     native_darwin_route_config_error_code(cleanup.error),
                 message);
}

void append_rollback_cleanup_failure(
    vpn_engine::ValidationResult *result,
    const NativeDarwinRouteConfigResult &cleanup) {
  if (!result || cleanup.ok())
    return;

  vpn_engine::ValidationResult cleanup_result =
      route_cleanup_failure_result(cleanup);
  result->message += "; rollback cleanup failed (" + cleanup_result.code + ")";
  if (!cleanup_result.message.empty())
    result->message += ": " + cleanup_result.message;
}

} // namespace

NativeDarwinPacketIoApi default_native_darwin_packet_io_api() {
  NativeDarwinPacketIoApi api;
#if defined(__APPLE__)
  api.set_nonblocking_fd = [](int fd) {
    const int flags = ::fcntl(fd, F_GETFL, 0);
    if (flags < 0)
      return errno_or_io_error();
    if (::fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0)
      return errno_or_io_error();
    return 0;
  };
  api.wait_readable_fd = [](int fd, int timeout_ms) {
    pollfd descriptor{};
    descriptor.fd = fd;
    descriptor.events = POLLIN;
    return ::poll(&descriptor, 1, timeout_ms);
  };
  api.wait_writable_fd = [](int fd, int timeout_ms) {
    pollfd descriptor{};
    descriptor.fd = fd;
    descriptor.events = POLLOUT;
    return ::poll(&descriptor, 1, timeout_ms);
  };
  api.read_fd = [](int fd, void *buffer, std::size_t size) {
    return static_cast<std::ptrdiff_t>(::read(fd, buffer, size));
  };
  api.write_fd = [](int fd, const void *buffer, std::size_t size) {
    return static_cast<std::ptrdiff_t>(::write(fd, buffer, size));
  };
  api.last_error = [] { return errno; };
#else
  api.set_nonblocking_fd = [](int) { return unsupported_native_api_error(); };
  api.wait_readable_fd = [](int, int) { return -1; };
  api.wait_writable_fd = [](int, int) { return -1; };
  api.read_fd = [](int, void *, std::size_t) {
    return static_cast<std::ptrdiff_t>(-1);
  };
  api.write_fd = [](int, const void *, std::size_t) {
    return static_cast<std::ptrdiff_t>(-1);
  };
  api.last_error = [] { return unsupported_native_api_error(); };
#endif
  return api;
}

std::unique_ptr<NativePacketDeviceUtunSession>
create_native_darwin_utun_packet_session(
    const vpn_engine::TunnelMetadata &metadata) {
  return std::unique_ptr<NativePacketDeviceUtunSession>(
      new RealUtunPacketSession(metadata));
}

std::unique_ptr<NativePacketDeviceUtunSession>
create_native_darwin_utun_packet_session(
    const vpn_engine::TunnelMetadata &metadata, NativeUtunApi utun_api,
    NativeDarwinPacketIoApi io_api) {
  return std::unique_ptr<NativePacketDeviceUtunSession>(
      new RealUtunPacketSession(metadata, std::move(utun_api),
                                std::move(io_api)));
}

NativePacketDeviceDependencies default_native_packet_device_dependencies() {
  NativePacketDeviceDependencies deps;
  deps.create_utun_session =
      [](const vpn_engine::TunnelMetadata &metadata) {
        return create_native_darwin_utun_packet_session(metadata);
      };
  deps.create_route_config = [](const NativeUtunMetadata &metadata) {
    return std::unique_ptr<NativePacketDeviceRouteConfig>(
        new RealRouteConfig(metadata));
  };
  return deps;
}
// End inlined from platform/darwin/native_packet_device_errors include-unit
// Begin inlined from platform/darwin/native_packet_device_public include-unit
NativePacketDevice::NativePacketDevice()
    : NativePacketDevice(default_native_packet_device_dependencies()) {}

NativePacketDevice::NativePacketDevice(
    NativePacketDeviceDependencies dependencies)
    : dependencies_(std::move(dependencies)) {}

NativePacketDevice::~NativePacketDevice() { close(); }

vpn_engine::ValidationResult
NativePacketDevice::open(const vpn_engine::DeviceConfig &config) {
  vpn_engine::ValidationResult closed = close_resources();
  if (!closed.ok)
    return closed;

  if (!dependencies_.create_utun_session) {
    return invalid("packet_device_api_missing",
                   "native Darwin packet device dependencies are incomplete");
  }

  // Build a minimal TunnelMetadata for the utun factory (it only needs
  // interface_name and mtu for device creation, not routes/DNS).
  vpn_engine::TunnelMetadata device_meta;
  device_meta.interface_name = config.interface_name;
  device_meta.mtu = config.mtu;

  std::unique_ptr<NativePacketDeviceUtunSession> utun =
      dependencies_.create_utun_session(device_meta);
  if (!utun)
    return invalid("packet_device_api_missing",
                   "native utun packet session factory returned null");

  NativeUtunStartResult started = utun->start();
  if (!started.ok())
    return utun_start_failure_result(started);

  // NOTE: No route_config is created or applied here.
  // Network routes and DNS should be applied separately via
  // PlatformNetworkOps::apply_tunnel_config() by the caller.

  utun_session_ = std::move(utun);
  open_ = true;
  return {};
}

vpn_engine::ValidationResult
NativePacketDevice::open(const vpn_engine::TunnelMetadata &metadata) {
  vpn_engine::ValidationResult closed = close_resources();
  if (!closed.ok)
    return closed;

  if (!dependencies_.create_utun_session ||
      !dependencies_.create_route_config) {
    return invalid("packet_device_api_missing",
                   "native Darwin packet device dependencies are incomplete");
  }

  std::unique_ptr<NativePacketDeviceUtunSession> utun =
      dependencies_.create_utun_session(metadata);
  if (!utun)
    return invalid("packet_device_api_missing",
                   "native utun packet session factory returned null");

  NativeUtunStartResult started = utun->start();
  if (!started.ok())
    return utun_start_failure_result(started);

  std::unique_ptr<NativePacketDeviceRouteConfig> route_config =
      dependencies_.create_route_config(started.metadata);
  if (!route_config) {
    utun->stop();
    return invalid("packet_device_api_missing",
                   "native Darwin route config factory returned null");
  }

  vpn_engine::TunnelMetadata configured_metadata = metadata;
  if (!started.metadata.interface_name.empty())
    configured_metadata.interface_name = started.metadata.interface_name;
  if (started.metadata.mtu > 0)
    configured_metadata.mtu = started.metadata.mtu;

  NativeDarwinRouteConfigResult configured =
      route_config->configure(configured_metadata);
  if (!configured.ok()) {
    NativeDarwinRouteConfigResult rollback_cleanup = route_config->cleanup();
    if (!rollback_cleanup.ok())
      route_config_ = std::move(route_config);
    utun->stop();
    vpn_engine::ValidationResult result =
        route_config_failure_result(configured);
    append_rollback_cleanup_failure(&result, rollback_cleanup);
    return result;
  }

  utun_session_ = std::move(utun);
  route_config_ = std::move(route_config);
  open_ = true;
  return {};
}

vpn_engine::ValidationResult
NativePacketDevice::read_packet(std::vector<std::uint8_t> *packet) {
  if (!packet)
    return invalid("packet_device_invalid_argument",
                   "packet output pointer is null");
  if (!open_ || !utun_session_)
    return invalid("packet_device_closed", "packet device is closed");

  std::vector<std::uint8_t> frame;
  vpn_engine::ValidationResult read = utun_session_->read_frame(&frame);
  if (!read.ok)
    return read;

  vpn_engine::ValidationResult validated =
      validate_read_frame(frame, nullptr);
  if (!validated.ok)
    return validated;

  packet->assign(frame.begin() + kUtunAddressFamilyHeaderSize, frame.end());
  return {};
}

vpn_engine::ValidationResult NativePacketDevice::write_packet(
    const std::vector<std::uint8_t> &packet) {
  if (!open_ || !utun_session_)
    return invalid("packet_device_closed", "packet device is closed");

  std::uint32_t family = 0;
  vpn_engine::ValidationResult selected =
      address_family_for_packet(packet, &family);
  if (!selected.ok)
    return selected;

  std::vector<std::uint8_t> frame;
  frame.reserve(packet.size() + kUtunAddressFamilyHeaderSize);
  append_address_family(&frame, family);
  frame.insert(frame.end(), packet.begin(), packet.end());
  return utun_session_->write_frame(frame);
}

vpn_engine::ValidationResult NativePacketDevice::close_resources() {
  NativeDarwinRouteConfigResult cleanup_result;
  bool cleanup_failed = false;

  // Routes are removed before stopping utun. If deletion fails, keep the route
  // config object so a later close() can retry, but still close the fd.
  if (route_config_) {
    cleanup_result = route_config_->cleanup();
    if (cleanup_result.ok()) {
      route_config_.reset();
    } else {
      cleanup_failed = true;
    }
  }

  if (utun_session_) {
    utun_session_->stop();
    utun_session_.reset();
  }

  open_ = false;
  if (cleanup_failed)
    return route_cleanup_failure_result(cleanup_result);
  return {};
}

void NativePacketDevice::close() { static_cast<void>(close_resources()); }

std::unique_ptr<vpn_engine::PacketDevice> create_native_packet_device() {
  return std::unique_ptr<vpn_engine::PacketDevice>(new NativePacketDevice());
}
// End inlined from platform/darwin/native_packet_device_public include-unit
} // namespace platform
} // namespace exv
