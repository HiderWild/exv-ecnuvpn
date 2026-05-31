#pragma once

#include <cstdint>
#include <functional>
#include <memory>
#include <string>

namespace ecnuvpn {
namespace platform {

enum class NativeWintunError {
  none,
  wintun_missing,
  dll_load_failed,
  api_missing,
  adapter_open_failed,
  session_start_failed,
  interface_index_failed,
};

const char *native_wintun_error_code(NativeWintunError error);

struct NativeWintunMetadata {
  std::wstring adapter_name;
  std::uint64_t luid = 0;
  std::uint32_t if_index = 0;
};

struct NativeWintunStartResult {
  NativeWintunError error = NativeWintunError::none;
  NativeWintunMetadata metadata;
  std::string detail;

  bool ok() const { return error == NativeWintunError::none; }
};

struct NativeWintunApi {
  using AdapterHandle = void *;
  using SessionHandle = void *;

  std::function<AdapterHandle(const std::wstring &)> open_adapter;
  std::function<AdapterHandle(const std::wstring &, const std::wstring &)>
      create_adapter;
  std::function<void(AdapterHandle)> close_adapter;
  std::function<bool(AdapterHandle, std::uint64_t &)> get_adapter_luid;
  std::function<bool(std::uint64_t, std::uint32_t &)> get_interface_index;
  std::function<SessionHandle(AdapterHandle, std::uint32_t)> start_session;
  std::function<void(SessionHandle)> end_session;
  std::shared_ptr<void> owner;
};

struct NativeWintunDependencies {
  std::function<std::wstring()> path_provider;
  std::function<bool(const std::wstring &)> file_exists;
  std::function<bool(const std::wstring &, NativeWintunApi &, std::string &)>
      api_loader;
};

NativeWintunDependencies default_native_wintun_dependencies();

struct NativeWintunConfig {
  std::wstring adapter_name_prefix = L"ECNUVPN-Native";
  std::wstring tunnel_type = L"ECNUVPN";
  std::uint32_t session_capacity = 0x400000;
};

class NativeWintun {
public:
  explicit NativeWintun(NativeWintunDependencies dependencies,
                        NativeWintunConfig config = {});
  ~NativeWintun();

  NativeWintun(const NativeWintun &) = delete;
  NativeWintun &operator=(const NativeWintun &) = delete;

  NativeWintunStartResult start();
  void stop();

  bool running() const;
  const NativeWintunMetadata &metadata() const;

private:
  NativeWintunDependencies dependencies_;
  NativeWintunConfig config_;
  NativeWintunApi api_;
  NativeWintunApi::AdapterHandle adapter_ = nullptr;
  NativeWintunApi::SessionHandle session_ = nullptr;
  NativeWintunMetadata metadata_;
};

std::wstring native_wintun_adapter_name(const std::wstring &prefix);

} // namespace platform
} // namespace ecnuvpn
