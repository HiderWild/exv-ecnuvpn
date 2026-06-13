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

