#include "platform/win32/native_wintun.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

bool starts_with(const std::wstring &value, const std::wstring &prefix) {
  return value.compare(0, prefix.size(), prefix) == 0;
}

struct MockWintun {
  std::wstring dll_path = L"C:\\bundle\\wintun.dll";
  bool dll_exists = true;
  bool loader_succeeds = true;
  bool open_returns_existing = false;
  std::uint64_t luid = 0x1234567812345678ULL;
  std::uint32_t if_index = 42;

  bool path_provider_called = false;
  std::wstring exists_checked_path;
  bool loader_called = false;
  std::wstring loaded_path;
  std::vector<std::wstring> opened_names;
  std::vector<std::wstring> created_names;
  std::vector<std::wstring> create_tunnel_types;
  std::uint64_t index_requested_luid = 0;
  int sessions_started = 0;
  int sessions_ended = 0;
  int adapters_deleted = 0;
  int adapters_closed = 0;

  int existing_adapter = 0;
  int created_adapter = 0;
  int session = 0;
};

ecnuvpn::platform::NativeWintunApi make_api(MockWintun &mock) {
  using Api = ecnuvpn::platform::NativeWintunApi;

  Api api;
  api.open_adapter = [&mock](const std::wstring &name) -> Api::AdapterHandle {
    mock.opened_names.push_back(name);
    if (mock.open_returns_existing)
      return &mock.existing_adapter;
    return nullptr;
  };
  api.create_adapter = [&mock](const std::wstring &name,
                               const std::wstring &tunnel_type)
      -> Api::AdapterHandle {
    mock.created_names.push_back(name);
    mock.create_tunnel_types.push_back(tunnel_type);
    return &mock.created_adapter;
  };
  api.close_adapter = [&mock](Api::AdapterHandle) { ++mock.adapters_closed; };
  api.get_adapter_luid = [&mock](Api::AdapterHandle,
                                 std::uint64_t &luid) -> bool {
    luid = mock.luid;
    return true;
  };
  api.get_interface_index = [&mock](std::uint64_t luid,
                                    std::uint32_t &if_index) -> bool {
    mock.index_requested_luid = luid;
    if_index = mock.if_index;
    return true;
  };
  api.start_session = [&mock](Api::AdapterHandle,
                              std::uint32_t) -> Api::SessionHandle {
    ++mock.sessions_started;
    return &mock.session;
  };
  api.end_session = [&mock](Api::SessionHandle) { ++mock.sessions_ended; };
  api.delete_adapter = [&mock](Api::AdapterHandle) {
    ++mock.adapters_deleted;
    return true;
  };
  return api;
}

ecnuvpn::platform::NativeWintunDependencies make_dependencies(
    MockWintun &mock) {
  ecnuvpn::platform::NativeWintunDependencies deps;
  deps.path_provider = [&mock] {
    mock.path_provider_called = true;
    return mock.dll_path;
  };
  deps.file_exists = [&mock](const std::wstring &path) {
    mock.exists_checked_path = path;
    return mock.dll_exists;
  };
  deps.api_loader = [&mock](const std::wstring &path,
                            ecnuvpn::platform::NativeWintunApi &api,
                            std::string &error) {
    mock.loader_called = true;
    mock.loaded_path = path;
    if (!mock.loader_succeeds) {
      error = "loader failed";
      return false;
    }
    api = make_api(mock);
    return true;
  };
  return deps;
}

ecnuvpn::platform::NativeWintunConfig config_with_prefix(
    const std::wstring &prefix) {
  ecnuvpn::platform::NativeWintunConfig config;
  config.adapter_name_prefix = prefix;
  config.tunnel_type = L"ECNUVPN";
  config.session_capacity = 0x200000;
  return config;
}

bool locates_bundled_wintun_path_through_provider() {
  MockWintun mock;
  mock.dll_path = L"C:\\Program Files\\ECNU-VPN\\bin\\wintun.dll";

  ecnuvpn::platform::NativeWintun wintun(make_dependencies(mock),
                                         config_with_prefix(L"ECNUVPN-D2"));
  auto result = wintun.start();

  bool ok = true;
  ok = expect(result.ok(), "mocked Wintun lifecycle should start") && ok;
  ok = expect(mock.path_provider_called,
              "path provider should be consulted for bundled wintun.dll") &&
       ok;
  ok = expect(mock.exists_checked_path == mock.dll_path,
              "bundled wintun.dll path should be checked without loading a real DLL") &&
       ok;
  ok = expect(mock.loader_called,
              "injected loader should be called when bundled DLL exists") &&
       ok;
  ok = expect(mock.loaded_path == mock.dll_path,
              "injected loader should receive the bundled wintun.dll path") &&
       ok;
  return ok;
}

bool returns_wintun_missing_when_dll_absent() {
  MockWintun mock;
  mock.dll_exists = false;

  ecnuvpn::platform::NativeWintun wintun(make_dependencies(mock),
                                         config_with_prefix(L"ECNUVPN-D2"));
  auto result = wintun.start();

  bool ok = true;
  ok = expect(!result.ok(), "start should fail when wintun.dll is absent") &&
       ok;
  ok = expect(result.error == ecnuvpn::platform::NativeWintunError::wintun_missing,
              "missing bundled DLL should report wintun_missing") &&
       ok;
  ok = expect(std::string(ecnuvpn::platform::native_wintun_error_code(
                  result.error)) == "wintun_missing",
              "missing bundled DLL should expose stable error code text") &&
       ok;
  ok = expect(!mock.loader_called,
              "loader should not run after bundled DLL missing check fails") &&
       ok;
  return ok;
}

bool creates_adapter_with_deterministic_name_prefix_when_open_missing() {
  MockWintun mock;
  mock.open_returns_existing = false;
  const std::wstring prefix = L"ECNUVPN-D2";

  ecnuvpn::platform::NativeWintun first(make_dependencies(mock),
                                        config_with_prefix(prefix));
  auto first_result = first.start();

  const std::wstring first_name =
      mock.created_names.empty() ? L"" : mock.created_names.back();

  ecnuvpn::platform::NativeWintun second(make_dependencies(mock),
                                         config_with_prefix(prefix));
  auto second_result = second.start();

  const std::wstring second_name =
      mock.created_names.empty() ? L"" : mock.created_names.back();

  bool ok = true;
  ok = expect(first_result.ok() && second_result.ok(),
              "mocked starts should succeed") &&
       ok;
  ok = expect(mock.opened_names.size() == 2,
              "adapter lookup should run before create") &&
       ok;
  ok = expect(mock.created_names.size() == 2,
              "missing adapter should be created") &&
       ok;
  ok = expect(starts_with(first_name, prefix),
              "created adapter name should use the configured deterministic prefix") &&
       ok;
  ok = expect(first_name == second_name,
              "same prefix should produce deterministic adapter name") &&
       ok;
  ok = expect(first_result.metadata.adapter_name == first_name &&
                  second_result.metadata.adapter_name == second_name,
              "reported metadata should include the adapter name") &&
       ok;
  return ok;
}

bool opens_existing_adapter_without_creating() {
  MockWintun mock;
  mock.open_returns_existing = true;

  ecnuvpn::platform::NativeWintun wintun(make_dependencies(mock),
                                         config_with_prefix(L"ECNUVPN-D2"));
  auto result = wintun.start();

  bool ok = true;
  ok = expect(result.ok(), "existing adapter should be opened") && ok;
  ok = expect(mock.opened_names.size() == 1,
              "existing adapter path should attempt open") &&
       ok;
  ok = expect(mock.created_names.empty(),
              "existing adapter path should not create a duplicate adapter") &&
       ok;
  ok = expect(starts_with(result.metadata.adapter_name, L"ECNUVPN-D2"),
              "opened adapter metadata should preserve deterministic prefix") &&
       ok;
  return ok;
}

bool reports_adapter_luid_and_if_index() {
  MockWintun mock;
  mock.luid = 0x9988776655443322ULL;
  mock.if_index = 77;

  ecnuvpn::platform::NativeWintun wintun(make_dependencies(mock),
                                         config_with_prefix(L"ECNUVPN-D2"));
  auto result = wintun.start();

  bool ok = true;
  ok = expect(result.ok(), "mocked Wintun lifecycle should start") && ok;
  ok = expect(result.metadata.luid == mock.luid,
              "metadata should report adapter LUID") &&
       ok;
  ok = expect(result.metadata.if_index == mock.if_index,
              "metadata should report adapter interface index") &&
       ok;
  ok = expect(mock.index_requested_luid == mock.luid,
              "interface index lookup should use the adapter LUID") &&
       ok;
  return ok;
}

bool stop_closes_active_session_once() {
  MockWintun mock;

  ecnuvpn::platform::NativeWintun wintun(make_dependencies(mock),
                                         config_with_prefix(L"ECNUVPN-D2"));
  auto result = wintun.start();
  wintun.stop();
  wintun.stop();

  bool ok = true;
  ok = expect(result.ok(), "mocked Wintun lifecycle should start") && ok;
  ok = expect(mock.sessions_started == 1,
              "start should create one Wintun session") &&
       ok;
  ok = expect(mock.sessions_ended == 1,
              "stop should close the active Wintun session exactly once") &&
       ok;
  ok = expect(mock.adapters_closed == 1,
              "stop should close the Wintun adapter exactly once") &&
       ok;
  ok = expect(!wintun.running(), "stop should clear running state") && ok;
  return ok;
}

bool delete_adapter_ends_session_deletes_adapter_and_closes_handle_once() {
  MockWintun mock;

  ecnuvpn::platform::NativeWintun wintun(make_dependencies(mock),
                                         config_with_prefix(L"ECNUVPN-D2"));
  auto result = wintun.start();
  auto deleted = wintun.delete_adapter();
  auto second_delete = wintun.delete_adapter();

  bool ok = true;
  ok = expect(result.ok(), "mocked Wintun lifecycle should start") && ok;
  ok = expect(deleted.ok(), "delete_adapter should succeed with mocked API") &&
       ok;
  ok = expect(second_delete.ok(), "delete_adapter should be idempotent") && ok;
  ok = expect(mock.sessions_ended == 1,
              "delete_adapter should end the active Wintun session once") &&
       ok;
  ok = expect(mock.adapters_deleted == 1,
              "delete_adapter should remove the adapter once") &&
       ok;
  ok = expect(mock.adapters_closed == 1,
              "delete_adapter should close the adapter handle once") &&
       ok;
  ok = expect(!wintun.running(), "delete_adapter should clear running state") &&
       ok;
  return ok;
}

} // namespace

namespace ecnuvpn {
namespace platform {

std::string get_bundled_wintun_path() { return ""; }

} // namespace platform
} // namespace ecnuvpn

int main() {
  bool ok = true;
  ok = locates_bundled_wintun_path_through_provider() && ok;
  ok = returns_wintun_missing_when_dll_absent() && ok;
  ok = creates_adapter_with_deterministic_name_prefix_when_open_missing() && ok;
  ok = opens_existing_adapter_without_creating() && ok;
  ok = reports_adapter_luid_and_if_index() && ok;
  ok = stop_closes_active_session_once() && ok;
  ok = delete_adapter_ends_session_deletes_adapter_and_closes_handle_once() && ok;
  return ok ? 0 : 1;
}
