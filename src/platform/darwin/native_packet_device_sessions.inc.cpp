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

