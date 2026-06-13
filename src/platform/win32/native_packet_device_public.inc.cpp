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

  if (!dependencies_.create_wintun_session)
    return invalid("packet_device_api_missing",
                   "native packet device dependencies are incomplete");

  std::unique_ptr<NativePacketDeviceWintunSession> wintun =
      dependencies_.create_wintun_session();
  if (!wintun)
    return invalid("packet_device_api_missing",
                   "native Wintun packet session factory returned null");

  NativeWintunStartResult started = wintun->start();
  if (!started.ok())
    return wintun_start_failure_result(started);

  // NOTE: No IP config is applied here.
  // Network address, routes, and DNS should be applied separately via
  // PlatformNetworkOps::apply_tunnel_config() by the caller.

  wintun_session_ = std::move(wintun);
  open_ = true;
  return {};
}

vpn_engine::ValidationResult
NativePacketDevice::open(const vpn_engine::TunnelMetadata &metadata) {
  vpn_engine::ValidationResult closed = close_resources();
  if (!closed.ok)
    return closed;

  if (!dependencies_.create_wintun_session || !dependencies_.create_ip_config)
    return invalid("packet_device_api_missing",
                   "native packet device dependencies are incomplete");

  std::unique_ptr<NativePacketDeviceWintunSession> wintun =
      dependencies_.create_wintun_session();
  if (!wintun)
    return invalid("packet_device_api_missing",
                   "native Wintun packet session factory returned null");

  NativeWintunStartResult started = wintun->start();
  if (!started.ok())
    return wintun_start_failure_result(started);

  std::unique_ptr<NativePacketDeviceIpConfig> ip_config =
      dependencies_.create_ip_config(started.metadata.if_index);
  if (!ip_config) {
    wintun->stop();
    return invalid("packet_device_api_missing",
                   "native IP config factory returned null");
  }

  vpn_engine::TunnelMetadata configured_metadata = metadata;
  configured_metadata.interface_index =
      static_cast<int>(started.metadata.if_index);
  if (configured_metadata.interface_name.empty())
    configured_metadata.interface_name = narrow_utf8(started.metadata.adapter_name);

  NativeIpConfigResult configured = ip_config->configure(configured_metadata);
  if (!configured.ok()) {
    NativeIpConfigResult rollback_cleanup = ip_config->cleanup();
    if (!rollback_cleanup.ok())
      ip_config_ = std::move(ip_config);
    wintun->stop();
    vpn_engine::ValidationResult result = ip_config_failure_result(configured);
    append_rollback_cleanup_failure(&result, rollback_cleanup);
    return result;
  }

  wintun_session_ = std::move(wintun);
  ip_config_ = std::move(ip_config);
  open_ = true;
  return {};
}

vpn_engine::ValidationResult
NativePacketDevice::read_packet(std::vector<std::uint8_t> *packet) {
  if (!packet)
    return invalid("packet_device_invalid_argument",
                   "packet output pointer is null");
  if (!open_ || !wintun_session_)
    return invalid("packet_device_closed", "packet device is closed");
  return wintun_session_->read_packet(packet);
}

vpn_engine::ValidationResult NativePacketDevice::write_packet(
    const std::vector<std::uint8_t> &packet) {
  if (!open_ || !wintun_session_)
    return invalid("packet_device_closed", "packet device is closed");
  return wintun_session_->write_packet(packet);
}

vpn_engine::ValidationResult NativePacketDevice::close_resources() {
  NativeIpConfigResult cleanup_result;
  bool cleanup_failed = false;

  // Route cleanup is attempted before stopping Wintun. If route deletion fails,
  // keep the IP config object so a later close() can retry, but still stop the
  // Wintun session below so adapter/session handles are not leaked indefinitely.
  if (ip_config_) {
    cleanup_result = ip_config_->cleanup();
    if (cleanup_result.ok()) {
      ip_config_.reset();
    } else {
      cleanup_failed = true;
    }
  }

  if (wintun_session_) {
    wintun_session_->stop();
    wintun_session_.reset();
  }

  open_ = false;
  if (cleanup_failed)
    return ip_config_cleanup_failure_result(cleanup_result);
  return {};
}

void NativePacketDevice::close() {
  static_cast<void>(close_resources());
}

std::unique_ptr<vpn_engine::PacketDevice> create_native_packet_device() {
  return std::unique_ptr<vpn_engine::PacketDevice>(new NativePacketDevice());
}
