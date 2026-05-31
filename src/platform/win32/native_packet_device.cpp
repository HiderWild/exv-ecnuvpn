#include "platform/win32/native_packet_device.hpp"

#include "utils.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <iphlpapi.h>

#include <cstring>
#include <limits>
#include <string>
#include <utility>

namespace ecnuvpn {
namespace platform {
namespace {

using AdapterHandle = NativeWintunApi::AdapterHandle;
using SessionHandle = NativeWintunApi::SessionHandle;

using WintunOpenAdapterFn = AdapterHandle(WINAPI *)(const wchar_t *);
using WintunCreateAdapterFn =
    AdapterHandle(WINAPI *)(const wchar_t *, const wchar_t *, const GUID *);
using WintunCloseAdapterFn = void(WINAPI *)(AdapterHandle);
using WintunGetAdapterLuidFn = void(WINAPI *)(AdapterHandle, NET_LUID *);
using WintunStartSessionFn = SessionHandle(WINAPI *)(AdapterHandle, DWORD);
using WintunEndSessionFn = void(WINAPI *)(SessionHandle);
using WintunReceivePacketFn = BYTE *(WINAPI *)(SessionHandle, DWORD *);
using WintunReleaseReceivePacketFn = void(WINAPI *)(SessionHandle, BYTE *);
using WintunAllocateSendPacketFn = BYTE *(WINAPI *)(SessionHandle, DWORD);
using WintunSendPacketFn = void(WINAPI *)(SessionHandle, BYTE *);

struct PacketWintunApi {
  WintunOpenAdapterFn open_adapter = nullptr;
  WintunCreateAdapterFn create_adapter = nullptr;
  WintunCloseAdapterFn close_adapter = nullptr;
  WintunGetAdapterLuidFn get_adapter_luid = nullptr;
  WintunStartSessionFn start_session = nullptr;
  WintunEndSessionFn end_session = nullptr;
  WintunReceivePacketFn receive_packet = nullptr;
  WintunReleaseReceivePacketFn release_receive_packet = nullptr;
  WintunAllocateSendPacketFn allocate_send_packet = nullptr;
  WintunSendPacketFn send_packet = nullptr;
};

vpn_engine::ValidationResult invalid(std::string code, std::string message) {
  vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

NativeWintunStartResult wintun_failure(NativeWintunError error,
                                       std::string detail = {}) {
  NativeWintunStartResult result;
  result.error = error;
  result.detail = std::move(detail);
  return result;
}

std::wstring widen_utf8(const std::string &value) {
  if (value.empty())
    return {};

  int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.c_str(),
                                 static_cast<int>(value.size()), nullptr, 0);
  if (size > 0) {
    std::wstring result(static_cast<std::size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.c_str(),
                        static_cast<int>(value.size()), &result[0], size);
    return result;
  }

  std::wstring fallback;
  fallback.reserve(value.size());
  for (unsigned char ch : value)
    fallback.push_back(static_cast<wchar_t>(ch));
  return fallback;
}

std::string narrow_utf8(const std::wstring &value) {
  if (value.empty())
    return {};

  int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.c_str(),
                                 static_cast<int>(value.size()), nullptr, 0,
                                 nullptr, nullptr);
  if (size > 0) {
    std::string result(static_cast<std::size_t>(size), '\0');
    WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.c_str(),
                        static_cast<int>(value.size()), &result[0], size,
                        nullptr, nullptr);
    return result;
  }

  std::string fallback;
  fallback.reserve(value.size());
  for (wchar_t ch : value)
    fallback.push_back(ch >= 0 && ch <= 0x7f ? static_cast<char>(ch) : '?');
  return fallback;
}

bool file_exists(const std::wstring &path) {
  if (path.empty())
    return false;

  DWORD attributes = GetFileAttributesW(path.c_str());
  return attributes != INVALID_FILE_ATTRIBUTES &&
         (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0;
}

template <typename Fn>
bool load_proc(HMODULE module, const char *name, Fn &target,
               std::string *error) {
  FARPROC proc = GetProcAddress(module, name);
  if (!proc) {
    if (error)
      *error = std::string("missing Wintun export: ") + name;
    return false;
  }
#if defined(__GNUC__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wcast-function-type"
#endif
  target = reinterpret_cast<Fn>(proc);
#if defined(__GNUC__)
#pragma GCC diagnostic pop
#endif
  return true;
}

bool load_packet_wintun_api(HMODULE module, PacketWintunApi *api,
                            std::string *error) {
  if (!module || !api)
    return false;

  return load_proc(module, "WintunOpenAdapter", api->open_adapter, error) &&
         load_proc(module, "WintunCreateAdapter", api->create_adapter,
                   error) &&
         load_proc(module, "WintunCloseAdapter", api->close_adapter, error) &&
         load_proc(module, "WintunGetAdapterLUID", api->get_adapter_luid,
                   error) &&
         load_proc(module, "WintunStartSession", api->start_session, error) &&
         load_proc(module, "WintunEndSession", api->end_session, error) &&
         load_proc(module, "WintunReceivePacket", api->receive_packet,
                   error) &&
         load_proc(module, "WintunReleaseReceivePacket",
                   api->release_receive_packet, error) &&
         load_proc(module, "WintunAllocateSendPacket",
                   api->allocate_send_packet, error) &&
         load_proc(module, "WintunSendPacket", api->send_packet, error);
}

std::string packet_error_message(const char *operation, DWORD system_error) {
  return std::string(operation) + " failed with Windows error " +
         std::to_string(static_cast<unsigned long>(system_error));
}

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

NativePacketDevice::NativePacketDevice()
    : NativePacketDevice(default_native_packet_device_dependencies()) {}

NativePacketDevice::NativePacketDevice(
    NativePacketDeviceDependencies dependencies)
    : dependencies_(std::move(dependencies)) {}

NativePacketDevice::~NativePacketDevice() { close(); }

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

} // namespace platform
} // namespace ecnuvpn
