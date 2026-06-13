vpn_engine::ValidationResult
wintun_start_failure_result(const NativeWintunStartResult &start) {
  return invalid(std::string("native_wintun_") +
                     native_wintun_error_code(start.error),
                 start.detail.empty() ? "failed to start native Wintun session"
                                      : start.detail);
}

vpn_engine::ValidationResult
ip_config_failure_result(const NativeIpConfigResult &config) {
  std::string message = config.message.empty()
                            ? "failed to configure native tunnel interface"
                            : config.message;
  if (!config.target.empty())
    message += ": " + config.target;
  return invalid(std::string("native_ip_config_") +
                     native_ip_config_error_code(config.error),
                 message);
}

vpn_engine::ValidationResult
ip_config_cleanup_failure_result(const NativeIpConfigResult &cleanup) {
  std::string message = cleanup.message.empty()
                            ? "failed to cleanup native tunnel interface"
                            : cleanup.message;
  if (!cleanup.target.empty())
    message += ": " + cleanup.target;
  return invalid(std::string("native_ip_config_") +
                     native_ip_config_error_code(cleanup.error),
                 message);
}

void append_rollback_cleanup_failure(vpn_engine::ValidationResult *result,
                                     const NativeIpConfigResult &cleanup) {
  if (!result || cleanup.ok())
    return;

  vpn_engine::ValidationResult cleanup_result =
      ip_config_cleanup_failure_result(cleanup);
  result->message += "; rollback cleanup failed (" + cleanup_result.code + ")";
  if (!cleanup_result.message.empty())
    result->message += ": " + cleanup_result.message;
}

} // namespace

NativePacketDeviceDependencies default_native_packet_device_dependencies() {
  NativePacketDeviceDependencies deps;
  deps.create_wintun_session = [] {
    return std::unique_ptr<NativePacketDeviceWintunSession>(
        new RealWintunPacketSession());
  };
  deps.create_ip_config = [](std::uint32_t interface_index) {
    return std::unique_ptr<NativePacketDeviceIpConfig>(
        new RealNativePacketIpConfig(interface_index));
  };
  return deps;
}

