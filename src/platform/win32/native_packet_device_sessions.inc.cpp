class RealWintunPacketSession final : public NativePacketDeviceWintunSession {
public:
  RealWintunPacketSession() = default;
  ~RealWintunPacketSession() override { stop(); }

  NativeWintunStartResult start() override {
    NativeWintunStartResult result;
    if (session_) {
      result.metadata = metadata_;
      return result;
    }

    const std::wstring dll_path = widen_utf8(utils::get_bundled_wintun_path());
    if (!file_exists(dll_path))
      return wintun_failure(NativeWintunError::wintun_missing,
                            "bundled wintun.dll is missing");

    module_ = LoadLibraryW(dll_path.c_str());
    if (!module_)
      return wintun_failure(NativeWintunError::dll_load_failed,
                            "LoadLibraryW failed for wintun.dll");

    std::string load_error;
    if (!load_packet_wintun_api(module_, &api_, &load_error)) {
      unload_module();
      return wintun_failure(NativeWintunError::api_missing, load_error);
    }

    const std::wstring adapter_name =
        native_wintun_adapter_name(config_.adapter_name_prefix);
    AdapterHandle adapter = api_.open_adapter(adapter_name.c_str());
    if (!adapter)
      adapter = api_.create_adapter(adapter_name.c_str(),
                                    config_.tunnel_type.c_str(), nullptr);
    if (!adapter) {
      unload_module();
      return wintun_failure(NativeWintunError::adapter_open_failed,
                            "failed to open or create Wintun adapter");
    }

    NET_LUID luid{};
    api_.get_adapter_luid(adapter, &luid);

    NET_IFINDEX if_index = 0;
    if (ConvertInterfaceLuidToIndex(&luid, &if_index) != NO_ERROR) {
      api_.close_adapter(adapter);
      unload_module();
      return wintun_failure(NativeWintunError::interface_index_failed,
                            "failed to resolve Wintun adapter interface index");
    }

    SessionHandle session =
        api_.start_session(adapter, config_.session_capacity);
    if (!session) {
      api_.close_adapter(adapter);
      unload_module();
      return wintun_failure(NativeWintunError::session_start_failed,
                            "failed to start Wintun session");
    }

    adapter_ = adapter;
    session_ = session;
    metadata_.adapter_name = adapter_name;
    metadata_.luid = static_cast<std::uint64_t>(luid.Value);
    metadata_.if_index = static_cast<std::uint32_t>(if_index);

    result.metadata = metadata_;
    return result;
  }

  vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!packet)
      return invalid("packet_device_invalid_argument",
                     "packet output pointer is null");
    if (!session_)
      return invalid("packet_device_closed", "packet device is closed");

    DWORD size = 0;
    BYTE *bytes = api_.receive_packet(session_, &size);
    if (!bytes) {
      DWORD error = GetLastError();
      if (error == ERROR_NO_MORE_ITEMS)
        return invalid("packet_device_empty", "no packet is queued");
      return invalid("wintun_receive_failed",
                     packet_error_message("WintunReceivePacket", error));
    }

    packet->assign(bytes, bytes + size);
    api_.release_receive_packet(session_, bytes);
    return {};
  }

  vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) override {
    if (!session_)
      return invalid("packet_device_closed", "packet device is closed");
    if (packet.empty())
      return invalid("packet_device_invalid_packet", "packet is empty");
    if (packet.size() > std::numeric_limits<DWORD>::max())
      return invalid("packet_device_invalid_packet", "packet is too large");

    BYTE *bytes =
        api_.allocate_send_packet(session_, static_cast<DWORD>(packet.size()));
    if (!bytes) {
      DWORD error = GetLastError();
      return invalid("wintun_send_failed",
                     packet_error_message("WintunAllocateSendPacket", error));
    }

    std::memcpy(bytes, packet.data(), packet.size());
    api_.send_packet(session_, bytes);
    return {};
  }

  void stop() override {
    if (session_ && api_.end_session)
      api_.end_session(session_);
    session_ = nullptr;

    if (adapter_ && api_.close_adapter)
      api_.close_adapter(adapter_);
    adapter_ = nullptr;

    metadata_ = {};
    api_ = {};
    unload_module();
  }

private:
  void unload_module() {
    if (module_)
      FreeLibrary(module_);
    module_ = nullptr;
  }

  NativeWintunConfig config_;
  PacketWintunApi api_;
  HMODULE module_ = nullptr;
  AdapterHandle adapter_ = nullptr;
  SessionHandle session_ = nullptr;
  NativeWintunMetadata metadata_;
};

class RealNativePacketIpConfig final : public NativePacketDeviceIpConfig {
public:
  explicit RealNativePacketIpConfig(std::uint32_t interface_index)
      : config_(default_native_ip_helper_api(), options(interface_index)) {}

  NativeIpConfigResult
  configure(const vpn_engine::TunnelMetadata &metadata) override {
    return config_.configure(metadata);
  }

  NativeIpConfigResult cleanup() override { return config_.cleanup(); }

private:
  static NativeIpConfigOptions options(std::uint32_t interface_index) {
    NativeIpConfigOptions opts;
    opts.interface_index = interface_index;
    return opts;
  }

  NativeIpConfig config_;
};

