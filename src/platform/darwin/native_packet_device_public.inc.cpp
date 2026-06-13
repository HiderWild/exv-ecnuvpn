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
