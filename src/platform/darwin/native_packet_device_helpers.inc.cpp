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

