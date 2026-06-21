#include "core/use_cases/system_status_use_cases.hpp"

#include "core/config/config_platform_view.hpp"
#include "core/connection/connection_attempt.hpp"
#include "core/vpn/vpn.hpp"
#include "helper/common/helper_client.hpp"
#include "helper/common/helper_connector.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/helper_service_manager.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"
#include "platform/common/status_models.hpp"

#include <algorithm>
#include <cwctype>
#include <cstdlib>
#include <filesystem>
#include <string>
#include <system_error>
#include <utility>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#else
#include <sys/stat.h>
#endif

namespace exv::core {
namespace {

template <typename ServiceResponse>
nlohmann::json service_operation_payload(const ServiceResponse &response,
                                         nlohmann::json service_status) {
  return nlohmann::json{{"operation",
                         {{"success", response.success},
                          {"exit_code", response.exit_code},
                          {"message", response.message}}},
                        {"service_status", std::move(service_status)}};
}

UseCaseResult fail_with_payload(const char *error_code, std::string message,
                                nlohmann::json payload) {
  UseCaseResult result = UseCaseResult::fail(error_code, std::move(message));
  result.payload = std::move(payload);
  return result;
}

template <typename ServiceResponse>
UseCaseResult service_op_result(const ServiceResponse &response,
                                const char *error_code,
                                const char *fallback_message) {
  nlohmann::json payload{{"operation",
                          {{"success", response.success},
                           {"exit_code", response.exit_code},
                           {"message", response.message}}},
                         {"service_status",
                          exv::platform::service_status_to_json(
                              exv::platform::current_service_status())}};
  if (response.success) {
    return UseCaseResult::ok(std::move(payload));
  }
  return fail_with_payload(
      error_code, response.message.empty() ? fallback_message : response.message,
      std::move(payload));
}

std::string env_value(const char *name) {
  const char *value = std::getenv(name);
  return value ? std::string(value) : std::string();
}

std::filesystem::path cli_install_dir() {
#ifdef _WIN32
  std::string local_app_data = env_value("LOCALAPPDATA");
  if (!local_app_data.empty()) {
    return std::filesystem::path(local_app_data) / "EXV" / "bin";
  }
  std::string user_profile = env_value("USERPROFILE");
  if (!user_profile.empty()) {
    return std::filesystem::path(user_profile) / "AppData" / "Local" /
           "EXV" / "bin";
  }
  return std::filesystem::temp_directory_path() / "EXV" / "bin";
#else
  std::string home = env_value("HOME");
  if (!home.empty()) {
    return std::filesystem::path(home) / ".local" / "bin";
  }
  return std::filesystem::temp_directory_path() / "exv-bin";
#endif
}

std::filesystem::path cli_target_path() {
#ifdef _WIN32
  return cli_install_dir() / "exv.exe";
#else
  return cli_install_dir() / "exv";
#endif
}

std::filesystem::path cli_source_path() {
  std::filesystem::path current(exv::platform::get_executable_path());
  std::vector<std::filesystem::path> candidates;
#ifdef _WIN32
  candidates.push_back(current.parent_path() / "bin" / "exv.exe");
  candidates.push_back(current.parent_path() / "exv.exe");
#else
  candidates.push_back(current.parent_path() / "bin" / "exv");
  candidates.push_back(current.parent_path() / "exv");
#endif
  candidates.push_back(current);

  for (const auto &candidate : candidates) {
    std::error_code ec;
    if (!std::filesystem::exists(candidate, ec) || ec) {
      continue;
    }
#ifdef _WIN32
    if (candidate.filename().wstring() == L"exv.exe") {
      return candidate;
    }
#else
    if (candidate.filename() == "exv") {
      return candidate;
    }
#endif
  }
  return current;
}

std::vector<std::string> split_path_env(const std::string &value) {
#ifdef _WIN32
  constexpr char delimiter = ';';
#else
  constexpr char delimiter = ':';
#endif
  std::vector<std::string> parts;
  std::string current;
  for (char ch : value) {
    if (ch == delimiter) {
      parts.push_back(current);
      current.clear();
    } else {
      current.push_back(ch);
    }
  }
  parts.push_back(current);
  return parts;
}

#ifdef _WIN32
std::wstring normalize_path_for_compare(std::filesystem::path path) {
  std::error_code ec;
  path = std::filesystem::weakly_canonical(path, ec);
  if (ec) {
    path = path.lexically_normal();
  }
  std::wstring text = path.wstring();
  while (!text.empty() && (text.back() == L'\\' || text.back() == L'/')) {
    text.pop_back();
  }
  std::transform(text.begin(), text.end(), text.begin(), [](wchar_t ch) {
    return static_cast<wchar_t>(std::towlower(ch));
  });
  return text;
}
#else
std::string normalize_path_for_compare(std::filesystem::path path) {
  std::error_code ec;
  path = std::filesystem::weakly_canonical(path, ec);
  if (ec) {
    path = path.lexically_normal();
  }
  std::string text = path.string();
  while (!text.empty() && text.back() == '/') {
    text.pop_back();
  }
  return text;
}
#endif

bool path_env_contains_dir(const std::filesystem::path &dir) {
  const auto expected = normalize_path_for_compare(dir);
  for (const auto &part : split_path_env(env_value("PATH"))) {
    if (part.empty()) {
      continue;
    }
    if (normalize_path_for_compare(std::filesystem::path(part)) == expected) {
      return true;
    }
  }
  return false;
}

#ifdef _WIN32
std::vector<std::wstring> split_windows_path(const std::wstring &value) {
  std::vector<std::wstring> parts;
  std::wstring current;
  for (wchar_t ch : value) {
    if (ch == L';') {
      if (!current.empty()) {
        parts.push_back(current);
      }
      current.clear();
    } else {
      current.push_back(ch);
    }
  }
  if (!current.empty()) {
    parts.push_back(current);
  }
  return parts;
}

std::wstring join_windows_path(const std::vector<std::wstring> &parts) {
  std::wstring result;
  for (const auto &part : parts) {
    if (part.empty()) {
      continue;
    }
    if (!result.empty()) {
      result += L';';
    }
    result += part;
  }
  return result;
}

bool update_user_path_with_cli_dir(bool add, std::string *warning) {
  HKEY key = nullptr;
  LONG open_result = RegCreateKeyExW(
      HKEY_CURRENT_USER, L"Environment", 0, nullptr, 0,
      KEY_QUERY_VALUE | KEY_SET_VALUE, nullptr, &key, nullptr);
  if (open_result != ERROR_SUCCESS) {
    if (warning) {
      *warning = "CLI copied, but user PATH could not be updated.";
    }
    return false;
  }

  DWORD type = 0;
  DWORD size = 0;
  LONG query_size = RegQueryValueExW(key, L"Path", nullptr, &type, nullptr, &size);
  std::wstring path_value;
  if (query_size == ERROR_SUCCESS &&
      (type == REG_SZ || type == REG_EXPAND_SZ) && size > 0) {
    path_value.resize(size / sizeof(wchar_t));
    LONG query = RegQueryValueExW(
        key, L"Path", nullptr, &type,
        reinterpret_cast<LPBYTE>(path_value.data()), &size);
    if (query == ERROR_SUCCESS) {
      while (!path_value.empty() && path_value.back() == L'\0') {
        path_value.pop_back();
      }
    } else {
      path_value.clear();
    }
  }

  const auto cli_dir = cli_install_dir();
  const std::wstring cli_dir_text = cli_dir.wstring();
  const std::wstring cli_dir_cmp = normalize_path_for_compare(cli_dir);
  std::vector<std::wstring> parts = split_windows_path(path_value);
  const auto matches_cli_dir = [&](const std::wstring &part) {
    return normalize_path_for_compare(std::filesystem::path(part)) == cli_dir_cmp;
  };

  const bool already_present =
      std::any_of(parts.begin(), parts.end(), matches_cli_dir);
  bool changed = false;
  if (add && !already_present) {
    parts.push_back(cli_dir_text);
    changed = true;
  } else if (!add && already_present) {
    parts.erase(std::remove_if(parts.begin(), parts.end(), matches_cli_dir),
                parts.end());
    changed = true;
  }

  if (changed) {
    std::wstring next = join_windows_path(parts);
    LONG set_result = RegSetValueExW(
        key, L"Path", 0, REG_EXPAND_SZ,
        reinterpret_cast<const BYTE *>(next.c_str()),
        static_cast<DWORD>((next.size() + 1) * sizeof(wchar_t)));
    if (set_result != ERROR_SUCCESS && warning) {
      *warning = "CLI copied, but user PATH could not be updated.";
    }
  }
  RegCloseKey(key);

  SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0,
                      reinterpret_cast<LPARAM>(L"Environment"),
                      SMTO_ABORTIFHUNG, 2000, nullptr);
  return true;
}
#endif

nlohmann::json cli_status_json(std::string warning = {}) {
  const auto target = cli_target_path();
  const auto source = cli_source_path();
  std::error_code ec;
  const bool installed = std::filesystem::exists(target, ec) && !ec;
  const bool available_in_path = path_env_contains_dir(cli_install_dir());
  if (warning.empty() && installed && !available_in_path) {
    warning = "CLI 已安装；请打开新终端让 PATH 生效。";
  }
  return nlohmann::json{{"installed", installed},
                        {"installPath", target.string()},
                        {"targetPath", source.string()},
                        {"availableInPath", available_in_path},
                        {"warning", warning}};
}

template <typename Fn>
UseCaseResult with_helper_service_lease(const std::string &purpose,
                                        bool bootstrap_oneshot,
                                        const std::string &preferred_mode,
                                        Fn &&fn) {
  exv::platform::BackendResolveOptions options;
  options.preferred_mode = preferred_mode;
  options.allow_oneshot = bootstrap_oneshot;
  options.allow_service_start = false;
  if (bootstrap_oneshot) {
    options.start_oneshot = true;
    const auto exv_path =
        std::filesystem::path(exv::platform::get_executable_path());
#ifdef _WIN32
    options.helper_path = (exv_path.parent_path() / "exv-helper.exe").string();
#else
    options.helper_path = (exv_path.parent_path() / "exv-helper").string();
#endif
  }

  nlohmann::json backend = exv::platform::resolve_backend(options);
  if (!backend.value("ok", false)) {
    return UseCaseResult::fail(
        backend.value("code", std::string("helper_unavailable")),
        backend.value(
            "message",
            std::string(
                "No helper instance is available for privileged service maintenance.")));
  }

  auto connector = exv::helper::HelperConnector::create();
  exv::helper::HelperConnectorConfig config;
  const std::string backend_mode = backend.value("backend", std::string());
  config.mode = backend_mode == "oneshot"
                    ? exv::helper::ConnectorMode::Transient
                    : exv::helper::ConnectorMode::Resident;
  config.pipe_endpoint = backend.value("endpoint", std::string());
  config.connect_timeout_ms = 500;

  auto client = connector->connect(config);
  if (!client || !client->is_connected()) {
    return UseCaseResult::fail(
        "helper_unavailable",
        "No helper instance is available for privileged service maintenance.");
  }

  (void)client->hello(exv::helper::HelloRequest{});

  exv::helper::AcquireCoreLeaseRequest acquire;
  acquire.core_pid = exv::connection_attempt::current_process_id();
  acquire.purpose = purpose;
  auto lease = client->acquire_core_lease(acquire);
  if (!lease.accepted || lease.lease_id.empty()) {
    return UseCaseResult::fail(
        "core_lease_unavailable",
        "Helper could not acquire a core lease for service maintenance.");
  }

  auto release = [&] {
    exv::helper::ReleaseCoreLeaseRequest release_req;
    release_req.lease_id = lease.lease_id;
    release_req.exit_if_oneshot = backend_mode == "oneshot";
    (void)client->release_core_lease(release_req);
  };

  UseCaseResult result = fn(*client);
  release();
  return result;
}

} // namespace

UseCaseResult finalize_service_uninstall_result(
    const exv::helper::UninstallServiceResponse &response,
    nlohmann::json service_status) {
  nlohmann::json payload =
      service_operation_payload(response, std::move(service_status));
  constexpr const char *kErrorCode = "service_uninstall_failed";
  constexpr const char *kFallbackMessage =
      "Helper service uninstallation failed.";

  if (!response.success) {
    return fail_with_payload(
        kErrorCode,
        response.message.empty() ? kFallbackMessage : response.message,
        std::move(payload));
  }

  if (!payload.value("service_status", nlohmann::json::object())
           .value("installed", true)) {
    return UseCaseResult::ok(std::move(payload));
  }

  return fail_with_payload(
      kErrorCode,
      "Helper service uninstallation did not remove the service registration.",
      std::move(payload));
}

SystemStatusUseCases::SystemStatusUseCases()
    : SystemStatusUseCases(exv::platform::get_config_dir()) {}

SystemStatusUseCases::SystemStatusUseCases(std::string config_dir)
    : manager_(std::move(config_dir)) {
  exv::platform::logging::configure_default_logging(false);
}

UseCaseResult SystemStatusUseCases::service_status() {
  return UseCaseResult::ok(exv::platform::service_status_to_json(
      exv::platform::current_service_status()));
}

UseCaseResult SystemStatusUseCases::helper_status() {
  exv::platform::BackendResolveOptions options;
  options.preferred_mode = "auto";
  options.allow_oneshot = true;
  options.allow_service_start = false;
  nlohmann::json resolved = exv::platform::resolve_backend(options);
  if (!resolved.value("ok", false)) {
    resolved["resolved"] = false;
    resolved["resolution_code"] = resolved.value("code", std::string());
    resolved["resolution_message"] = resolved.value("message", std::string());
    resolved["ok"] = true;
  } else {
    resolved["resolved"] = true;
  }
  return UseCaseResult::ok(resolved);
}

UseCaseResult SystemStatusUseCases::runtime_status() {
  exv::Config cfg = manager_.load();
  return UseCaseResult::ok(exv::platform::runtime_status_json(
      exv::config::to_platform_config_view(cfg)));
}

UseCaseResult SystemStatusUseCases::driver_status() {
  exv::Config cfg = manager_.load();
  return UseCaseResult::ok(exv::platform::driver_status_json(
      exv::config::to_platform_config_view(cfg)));
}

UseCaseResult
SystemStatusUseCases::install_driver(const nlohmann::json &payload) {
  exv::Config cfg = manager_.load();
  nlohmann::json result = exv::platform::install_driver(
      exv::config::to_platform_config_view(cfg), payload);
  if (result.is_object() && result.value("ok", true) == false) {
    return UseCaseResult::fail(result.value("code", "driver_install_failed"),
                               result.value("error", "Driver install failed"));
  }
  return UseCaseResult::ok(result);
}

UseCaseResult SystemStatusUseCases::cli_status() {
  return UseCaseResult::ok(cli_status_json());
}

UseCaseResult SystemStatusUseCases::install_cli() {
  const auto source = cli_source_path();
  const auto target = cli_target_path();
  std::error_code ec;
  if (!std::filesystem::exists(source, ec) || ec) {
    return UseCaseResult::fail("cli_source_missing",
                               "CLI source executable was not found.");
  }

  std::string warning;
  std::filesystem::create_directories(target.parent_path(), ec);
  if (ec) {
    return UseCaseResult::fail("cli_install_failed",
                               "Failed to create CLI install directory.");
  }

  const bool already_target = std::filesystem::equivalent(source, target, ec);
  if (ec) {
    ec.clear();
  }
  if (!already_target) {
    std::filesystem::copy_file(
        source, target, std::filesystem::copy_options::overwrite_existing, ec);
    if (ec) {
      return UseCaseResult::fail("cli_install_failed",
                                 "Failed to copy CLI executable: " +
                                     ec.message());
    }
  }

#ifdef _WIN32
  (void)update_user_path_with_cli_dir(true, &warning);
#else
  chmod(target.c_str(), 0755);
  if (!path_env_contains_dir(target.parent_path())) {
    warning = "CLI copied to ~/.local/bin; add it to PATH if exv is not found.";
  }
#endif

  return UseCaseResult::ok(cli_status_json(std::move(warning)));
}

UseCaseResult SystemStatusUseCases::uninstall_cli() {
  const auto target = cli_target_path();
  std::error_code ec;
  if (std::filesystem::exists(target, ec) && !ec) {
    std::filesystem::remove(target, ec);
    if (ec) {
      return UseCaseResult::fail("cli_uninstall_failed",
                                 "Failed to remove CLI executable: " +
                                     ec.message());
    }
  }

#ifdef _WIN32
  std::string warning;
  (void)update_user_path_with_cli_dir(false, &warning);
  return UseCaseResult::ok(cli_status_json(std::move(warning)));
#else
  return UseCaseResult::ok(cli_status_json());
#endif
}

UseCaseResult SystemStatusUseCases::install_helper() {
  return with_helper_service_lease("service.install", true, "auto", [](auto &client) {
    return service_op_result(
        client.install_service(exv::helper::InstallServiceRequest{}),
        "service_install_failed", "Helper service installation failed.");
  });
}

UseCaseResult SystemStatusUseCases::uninstall_helper() {
  exv::Config cfg = manager_.load();
  auto runtime = exv::vpn::read_runtime_status_snapshot(cfg);
  if (runtime.running || runtime.network_ready) {
    return UseCaseResult::fail(
        "vpn_session_active",
        "Disconnect the VPN session before uninstalling the helper service.");
  }
  auto self_cleanup = with_helper_service_lease(
      "service.uninstall", false, "service", [](auto &client) {
        auto response =
            client.uninstall_service(exv::helper::UninstallServiceRequest{});
        return finalize_service_uninstall_result(
            response, exv::platform::service_status_to_json(
                          exv::platform::current_service_status()));
      });
  if (self_cleanup.success || self_cleanup.error_code == "vpn_session_active") {
    return self_cleanup;
  }
  return with_helper_service_lease(
      "service.uninstall.fallback", true, "oneshot", [](auto &client) {
        auto response =
            client.uninstall_service(exv::helper::UninstallServiceRequest{});
        return finalize_service_uninstall_result(
            response, exv::platform::service_status_to_json(
                          exv::platform::current_service_status()));
      });
}

UseCaseResult SystemStatusUseCases::repair_helper() {
  return with_helper_service_lease("service.repair", true, "oneshot", [](auto &client) {
    return service_op_result(
        client.repair_service(exv::helper::RepairServiceRequest{}),
        "service_repair_failed", "Helper service repair failed.");
  });
}

} // namespace exv::core
