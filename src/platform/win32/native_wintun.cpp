#include "platform/win32/native_wintun.hpp"

#include "utils.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <iphlpapi.h>

#include <algorithm>
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
using WintunDeleteAdapterFn = BOOL(WINAPI *)(AdapterHandle, BOOL, BOOL *);

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

bool default_file_exists(const std::wstring &path) {
  if (path.empty())
    return false;

  DWORD attributes = GetFileAttributesW(path.c_str());
  return attributes != INVALID_FILE_ATTRIBUTES &&
         (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0;
}

template <typename Fn>
bool load_proc(HMODULE module, const char *name, Fn &target,
               std::string &error) {
  FARPROC proc = GetProcAddress(module, name);
  if (!proc) {
    error = std::string("missing Wintun export: ") + name;
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

bool load_real_wintun_api(const std::wstring &dll_path, NativeWintunApi &api,
                          std::string &error) {
  HMODULE module = LoadLibraryW(dll_path.c_str());
  if (!module) {
    error = "LoadLibraryW failed for wintun.dll";
    return false;
  }

  auto owner = std::shared_ptr<void>(
      module, [](void *handle) { FreeLibrary(static_cast<HMODULE>(handle)); });

  WintunOpenAdapterFn open_adapter = nullptr;
  WintunCreateAdapterFn create_adapter = nullptr;
  WintunCloseAdapterFn close_adapter = nullptr;
  WintunGetAdapterLuidFn get_adapter_luid = nullptr;
  WintunStartSessionFn start_session = nullptr;
  WintunEndSessionFn end_session = nullptr;
  WintunDeleteAdapterFn delete_adapter = nullptr;

  if (!load_proc(module, "WintunOpenAdapter", open_adapter, error) ||
      !load_proc(module, "WintunCreateAdapter", create_adapter, error) ||
      !load_proc(module, "WintunCloseAdapter", close_adapter, error) ||
      !load_proc(module, "WintunGetAdapterLUID", get_adapter_luid, error) ||
      !load_proc(module, "WintunStartSession", start_session, error) ||
      !load_proc(module, "WintunEndSession", end_session, error) ||
      !load_proc(module, "WintunDeleteAdapter", delete_adapter, error)) {
    return false;
  }

  api.open_adapter = [open_adapter](const std::wstring &name) {
    return open_adapter(name.c_str());
  };
  api.create_adapter = [create_adapter](const std::wstring &name,
                                        const std::wstring &tunnel_type) {
    return create_adapter(name.c_str(), tunnel_type.c_str(), nullptr);
  };
  api.close_adapter = [close_adapter](AdapterHandle adapter) {
    close_adapter(adapter);
  };
  api.get_adapter_luid = [get_adapter_luid](AdapterHandle adapter,
                                            std::uint64_t &value) {
    NET_LUID luid{};
    get_adapter_luid(adapter, &luid);
    value = static_cast<std::uint64_t>(luid.Value);
    return true;
  };
  api.get_interface_index = [](std::uint64_t luid_value,
                               std::uint32_t &if_index) {
    NET_LUID luid{};
    luid.Value = luid_value;
    NET_IFINDEX index = 0;
    if (ConvertInterfaceLuidToIndex(&luid, &index) != NO_ERROR)
      return false;
    if_index = static_cast<std::uint32_t>(index);
    return true;
  };
  api.start_session = [start_session](AdapterHandle adapter,
                                      std::uint32_t capacity) {
    return start_session(adapter, static_cast<DWORD>(capacity));
  };
  api.end_session = [end_session](SessionHandle session) {
    end_session(session);
  };
  api.delete_adapter = [delete_adapter](AdapterHandle adapter) {
    BOOL reboot_required = FALSE;
    return delete_adapter(adapter, TRUE, &reboot_required) != FALSE;
  };
  api.owner = std::move(owner);
  return true;
}

bool has_required_api(const NativeWintunApi &api) {
  return static_cast<bool>(api.open_adapter) &&
         static_cast<bool>(api.create_adapter) &&
         static_cast<bool>(api.close_adapter) &&
         static_cast<bool>(api.get_adapter_luid) &&
         static_cast<bool>(api.get_interface_index) &&
         static_cast<bool>(api.start_session) &&
         static_cast<bool>(api.end_session) &&
         static_cast<bool>(api.delete_adapter);
}

NativeWintunStartResult failure(NativeWintunError error,
                                const std::string &detail = {}) {
  NativeWintunStartResult result;
  result.error = error;
  result.detail = detail;
  return result;
}

void close_adapter(NativeWintunApi &api, AdapterHandle adapter) {
  if (adapter && api.close_adapter)
    api.close_adapter(adapter);
}

} // namespace

const char *native_wintun_error_code(NativeWintunError error) {
  switch (error) {
  case NativeWintunError::none:
    return "none";
  case NativeWintunError::wintun_missing:
    return "wintun_missing";
  case NativeWintunError::dll_load_failed:
    return "dll_load_failed";
  case NativeWintunError::api_missing:
    return "api_missing";
  case NativeWintunError::adapter_open_failed:
    return "adapter_open_failed";
  case NativeWintunError::session_start_failed:
    return "session_start_failed";
  case NativeWintunError::adapter_delete_failed:
    return "adapter_delete_failed";
  case NativeWintunError::interface_index_failed:
    return "interface_index_failed";
  }
  return "unknown";
}

NativeWintunDependencies default_native_wintun_dependencies() {
  NativeWintunDependencies dependencies;
  dependencies.path_provider = [] {
    return widen_utf8(utils::get_bundled_wintun_path());
  };
  dependencies.file_exists = default_file_exists;
  dependencies.api_loader = load_real_wintun_api;
  return dependencies;
}

NativeWintun::NativeWintun(NativeWintunDependencies dependencies,
                           NativeWintunConfig config)
    : dependencies_(std::move(dependencies)), config_(std::move(config)) {}

NativeWintun::~NativeWintun() { stop(); }

NativeWintunStartResult NativeWintun::start() {
  NativeWintunStartResult result;
  if (session_) {
    result.metadata = metadata_;
    return result;
  }

  if (!dependencies_.path_provider || !dependencies_.file_exists ||
      !dependencies_.api_loader)
    return failure(NativeWintunError::api_missing,
                   "native Wintun dependencies are incomplete");

  const std::wstring dll_path = dependencies_.path_provider();
  if (dll_path.empty() || !dependencies_.file_exists(dll_path))
    return failure(NativeWintunError::wintun_missing,
                   "bundled wintun.dll is missing");

  NativeWintunApi api;
  std::string load_error;
  if (!dependencies_.api_loader(dll_path, api, load_error))
    return failure(NativeWintunError::dll_load_failed, load_error);

  if (!has_required_api(api))
    return failure(NativeWintunError::api_missing,
                   "loaded Wintun API table is incomplete");

  const std::wstring adapter_name =
      native_wintun_adapter_name(config_.adapter_name_prefix);
  AdapterHandle adapter = api.open_adapter(adapter_name);
  if (!adapter)
    adapter = api.create_adapter(adapter_name, config_.tunnel_type);
  if (!adapter)
    return failure(NativeWintunError::adapter_open_failed,
                   "failed to open or create Wintun adapter");

  std::uint64_t luid = 0;
  if (!api.get_adapter_luid(adapter, luid)) {
    close_adapter(api, adapter);
    return failure(NativeWintunError::adapter_open_failed,
                   "failed to query Wintun adapter LUID");
  }

  std::uint32_t if_index = 0;
  if (!api.get_interface_index(luid, if_index)) {
    close_adapter(api, adapter);
    return failure(NativeWintunError::interface_index_failed,
                   "failed to resolve Wintun adapter interface index");
  }

  SessionHandle session = api.start_session(adapter, config_.session_capacity);
  if (!session) {
    close_adapter(api, adapter);
    return failure(NativeWintunError::session_start_failed,
                   "failed to start Wintun session");
  }

  api_ = std::move(api);
  adapter_ = adapter;
  session_ = session;
  metadata_.adapter_name = adapter_name;
  metadata_.luid = luid;
  metadata_.if_index = if_index;

  result.metadata = metadata_;
  return result;
}

void NativeWintun::stop() {
  if (session_ && api_.end_session)
    api_.end_session(session_);
  session_ = nullptr;

  if (adapter_ && api_.close_adapter)
    api_.close_adapter(adapter_);
  adapter_ = nullptr;

  api_ = {};
  metadata_ = {};
}

NativeWintunStartResult NativeWintun::delete_adapter() {
  if (!adapter_)
    return {};

  if (session_ && api_.end_session)
    api_.end_session(session_);
  session_ = nullptr;

  if (!api_.delete_adapter || !api_.delete_adapter(adapter_)) {
    return failure(NativeWintunError::adapter_delete_failed,
                   "failed to delete Wintun adapter");
  }

  if (api_.close_adapter)
    api_.close_adapter(adapter_);
  adapter_ = nullptr;
  api_ = {};
  metadata_ = {};
  return {};
}

bool NativeWintun::running() const { return session_ != nullptr; }

const NativeWintunMetadata &NativeWintun::metadata() const {
  return metadata_;
}

std::wstring native_wintun_adapter_name(const std::wstring &prefix) {
  const std::wstring normalized =
      prefix.empty() ? NativeWintunConfig{}.adapter_name_prefix : prefix;
  const std::wstring suffix = L"-Wintun";
  if (normalized.size() >= suffix.size() &&
      normalized.compare(normalized.size() - suffix.size(), suffix.size(),
                         suffix) == 0)
    return normalized;
  return normalized + suffix;
}

} // namespace platform
} // namespace ecnuvpn
