#include "app_api.hpp"

#include "config.hpp"
#include "config_api.hpp"
#include "config_manager.hpp"
#include "crypto.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "utils.hpp"
#include "virtual_network.hpp"
#include "vpn.hpp"

#include <algorithm>
#include <cstdio>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#ifndef _WIN32
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#else
#include <windows.h>
#endif

namespace ecnuvpn {
namespace app_api {
namespace {

#ifndef _WIN32
constexpr const char *kHelperSocketPath = "/var/run/exv-helper.sock";
#endif

// Unified error type strings matching the TypeScript VpnErrorType enum.
static constexpr const char *kErrorElevationRequired = "elevation_required";
static constexpr const char *kErrorElevationDenied    = "elevation_denied";
static constexpr const char *kErrorRuntimeMissing     = "runtime_missing";
static constexpr const char *kErrorConfigInvalid      = "config_invalid";
static constexpr const char *kErrorServiceMissing     = "service_missing";
static constexpr const char *kErrorNativeFailure      = "native_failure";
static constexpr const char *kErrorParseFailure       = "parse_failure";
static constexpr const char *kErrorUnknownAction      = "unknown_action";

nlohmann::json structured_error(const char *error_type, const std::string &message,
                                bool recoverable = true,
                                const std::string &recommended_action = "") {
  return nlohmann::json{{"ok", false},
                        {"error_type", error_type},
                        {"message", message},
                        {"recoverable", recoverable},
                        {"recommended_action", recommended_action}};
}

config::ConfigManager make_config_manager() {
  utils::ensure_dir(utils::get_config_dir());
  // The desktop / RPC entrypoint does not go through the CLI wizard, so we
  // have to make sure the encryption key file exists before any password
  // operation. Without this, config_set_password would always fail with
  // "Encryption key is missing" on a fresh install.
  crypto::init_key_if_needed();
  logger::init();
  return config::ConfigManager(utils::get_config_dir());
}

nlohmann::json send_helper_request(const nlohmann::json &request) {
  std::string payload = request.dump();
  payload.push_back('\n');
  std::string raw;

#ifdef _WIN32
  HANDLE hPipe = CreateFileA("\\\\.\\pipe\\exv-helper",
                             GENERIC_READ | GENERIC_WRITE, 0, NULL,
                             OPEN_EXISTING, 0, NULL);
  if (hPipe == INVALID_HANDLE_VALUE) {
    return structured_error(kErrorServiceMissing,
                            "Helper daemon not available",
                            /*recoverable=*/true,
                            "Install the helper service");
  }

  DWORD bytesWritten = 0;
  if (!WriteFile(hPipe, payload.c_str(), static_cast<DWORD>(payload.size()),
                 &bytesWritten, NULL) ||
      bytesWritten != payload.size()) {
    CloseHandle(hPipe);
    return structured_error(kErrorNativeFailure,
                            "Failed to send helper request",
                            /*recoverable=*/true,
                            "Retry the operation");
  }

  char buffer[1024];
  DWORD bytesRead = 0;
  while (ReadFile(hPipe, buffer, sizeof(buffer), &bytesRead, NULL) &&
         bytesRead > 0) {
    raw.append(buffer, bytesRead);
    if (raw.find('\n') != std::string::npos)
      break;
  }
  CloseHandle(hPipe);
#else
  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0) {
    return structured_error(kErrorNativeFailure,
                            "Failed to create socket",
                            /*recoverable=*/true,
                            "Retry the operation");
  }

  sockaddr_un addr {};
  addr.sun_family = AF_UNIX;
  std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", kHelperSocketPath);

  if (connect(fd, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) != 0) {
    close(fd);
    return structured_error(kErrorServiceMissing,
                            "Helper daemon not available",
                            /*recoverable=*/true,
                            "Install the helper service");
  }

  if (write(fd, payload.data(), payload.size()) !=
      static_cast<ssize_t>(payload.size())) {
    close(fd);
    return structured_error(kErrorNativeFailure,
                            "Failed to send helper request",
                            /*recoverable=*/true,
                            "Retry the operation");
  }
  shutdown(fd,SHUT_WR);

  char buffer[1024];
  ssize_t n;
  while ((n = read(fd, buffer, sizeof(buffer))) > 0) {
    raw.append(buffer, static_cast<size_t>(n));
    if (raw.find('\n') != std::string::npos)
      break;
  }
  close(fd);
#endif

  auto nl = raw.find('\n');
  if (nl != std::string::npos)
    raw.resize(nl);
  raw = utils::trim(raw);

  if (raw.empty()) {
    return structured_error(kErrorParseFailure,
                            "Empty helper response",
                            /*recoverable=*/true,
                            "Retry the operation");
  }

  try {
    return nlohmann::json::parse(raw);
  } catch (...) {
    return structured_error(kErrorParseFailure,
                            "Failed to parse helper response",
                            /*recoverable=*/true,
                            "Retry the operation");
  }
}

nlohmann::json frontend_status_from_helper(const nlohmann::json &helper_resp,
                                           const Config &cfg) {
  nlohmann::json j;
  j["connected"] = helper_resp.value("running", false);
  j["mode"] = helper_resp.value("running", false) ? "helper" : "disconnected";
  j["server"] = helper_resp.value("server", cfg.server);
  j["username"] = cfg.username;
  j["pid"] = helper_resp.value("pid", -1);
  j["supervisor_pid"] = helper_resp.value("supervisor_pid", -1);
  j["network_ready"] = helper_resp.value("network_ready", false);
  j["interface"] = helper_resp.value("interface", "");
  j["internal_ip"] = helper_resp.value("internal_ip", "");
  j["route_count"] = helper_resp.value("route_count",
                                        static_cast<int>(cfg.routes.size()));
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = helper_resp.value("rx_bytes", 0);
  j["tx_bytes"] = helper_resp.value("tx_bytes", 0);
  virtual_network::add_status_fields(j, j.value("interface", std::string()));
  return j;
}

nlohmann::json disconnected_status(const Config &cfg) {
  return frontend_status_from_helper(nlohmann::json{{"running", false}}, cfg);
}

nlohmann::json auth_config(const Config &cfg) {
  // Never echo the stored ciphertext or a fake mask back to the UI. The UI
  // shows a placeholder when password_stored is true and treats an empty
  // submitted password as "keep the existing one".
  return nlohmann::json{{"server", cfg.server},
                        {"username", cfg.username},
                        {"password", ""},
                        {"password_stored", !cfg.password.empty()},
                        {"user_agent", cfg.useragent},
                        {"remember_password", cfg.remember_password}};
}

nlohmann::json settings_config(const Config &cfg) {
  std::string extra_args;
  for (size_t i = 0; i < cfg.extra_args.size(); ++i) {
    if (i > 0)
      extra_args += " ";
    extra_args += cfg.extra_args[i];
  }

  return nlohmann::json{{"mtu", cfg.mtu},
                        {"dtls", !cfg.disable_dtls},
                        {"extra_args", extra_args},
                        {"log_path", cfg.log_file},
                        {"webui_port", cfg.webui_port},
                        {"webui_host", cfg.webui_bind},
                        {"webui_enabled", cfg.webui_enabled},
                        {"openconnect_runtime", cfg.openconnect_runtime},
                        {"windows_tunnel_driver", cfg.windows_tunnel_driver},
                        {"windows_tap_interface", cfg.windows_tap_interface}};
}

nlohmann::json routes_json(const Config &cfg) {
  nlohmann::json arr = nlohmann::json::array();
  for (const auto &route : cfg.routes) {
    arr.push_back({{"cidr", route}});
  }
  return arr;
}

nlohmann::json key_status_json() {
  std::string status = config_api::key_status();
  return nlohmann::json{{"present", status == "valid"},
                        {"fingerprint", status == "valid"
                                            ? nlohmann::json("active")
                                            : nlohmann::json(nullptr)},
                        {"status", status}};
}

nlohmann::json service_status_json() {
  bool available = helper::is_available();
  nlohmann::json j;
#ifdef __APPLE__
  j["installed"] =
      utils::file_exists("/Library/LaunchDaemons/com.ecnu.exv.helper.plist");
  j["path"] = "/usr/local/bin/exv";
#elif defined(__linux__)
  j["installed"] = utils::file_exists("/etc/systemd/system/exv-helper.service");
  j["path"] = "/usr/local/bin/exv";
#elif defined(_WIN32)
  j["path"] = "C:\\Program Files\\ECNU-VPN\\exv.exe";
  SC_HANDLE scm = OpenSCManagerA(NULL, NULL, SC_MANAGER_CONNECT);
  if (scm) {
    SC_HANDLE svc = OpenServiceA(scm, "exv-helper", SERVICE_QUERY_STATUS);
    j["installed"] = (svc != NULL);
    if (svc)
      CloseServiceHandle(svc);
    CloseServiceHandle(scm);
  } else {
    j["installed"] = false;
  }
#endif
  j["running"] = available;
  j["available"] = available;
  return j;
}

std::string first_nonempty_line(const std::string &text) {
  for (const auto &line : utils::split_lines(text)) {
    if (!line.empty())
      return line;
  }
  return "";
}

std::string openconnect_version(const std::string &path) {
  if (path.empty())
    return "";
#ifdef _WIN32
  return first_nonempty_line(
      utils::run_command_output(utils::shell_quote(path) + " --version 2>nul"));
#else
  return first_nonempty_line(
      utils::run_command_output(utils::shell_quote(path) + " --version 2>/dev/null"));
#endif
}

nlohmann::json runtime_status_json(const Config &cfg) {
  std::string bundled_path = utils::get_bundled_openconnect_path();
  std::string system_path = utils::get_openconnect_path("system");
  std::string resolved_path = utils::get_openconnect_path(cfg.openconnect_runtime);

  std::string source = "missing";
  if (!bundled_path.empty() && resolved_path == bundled_path) {
    source = "bundled";
  } else if (!system_path.empty() && resolved_path == system_path) {
    source = "system";
  }

  nlohmann::json j{
      {"mode", cfg.openconnect_runtime},
      {"available", !resolved_path.empty()},
      {"source", source},
      {"path", resolved_path},
      {"bundled_path", bundled_path},
      {"system_path", system_path},
      {"version", openconnect_version(resolved_path)},
      {"bundled_runtime_dir", utils::get_bundled_runtime_dir()},
  };

#ifdef _WIN32
  std::string wintun_path = utils::get_bundled_wintun_path();
  std::string tap_installer_path = utils::get_bundled_tap_installer_path();
  j["wintun_path"] = wintun_path;
  j["tap_installer_path"] = tap_installer_path;
  j["wintun_missing"] = wintun_path.empty();
  j["tap_missing"] = tap_installer_path.empty();
#endif

  if (resolved_path.empty()) {
    j["missing_what"] = "openconnect binary";
    j["recommended_action"] =
        "Reinstall the desktop package with the bundled OpenConnect runtime assets.";
    j["effect_on_connect"] =
        "VPN connection will fail with runtime_missing error.";
  }

  return j;
}

#ifdef _WIN32
std::vector<std::string> list_windows_adapters(const std::string &kind) {
  std::string filter;
  if (kind == "wintun") {
    filter =
        "($_.NetConnectionID -like '*Wintun*' -or $_.Name -like '*Wintun*' -or $_.Description -like '*Wintun*')";
  } else {
    filter =
        "($_.NetConnectionID -like '*TAP*' -or $_.Name -like '*TAP*' -or $_.Description -like '*TAP-Windows*' -or $_.Description -like '*tap0901*')";
  }

  std::string command =
      "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "
      "\"Get-CimInstance Win32_NetworkAdapter | Where-Object { "
      "$_.NetEnabled -ne $false -and " +
      filter + " } | ForEach-Object { "
      "if ($_.NetConnectionID) { $_.NetConnectionID } elseif ($_.Name) { $_.Name } }\"";

  return utils::split_lines(utils::run_command_output(command));
}

std::string effective_windows_driver(const Config &cfg,
                                     const std::vector<std::string> &tap_adapters,
                                     bool has_bundled_wintun) {
  if (cfg.windows_tunnel_driver == "tap")
    return "tap";
  if (cfg.windows_tunnel_driver == "wintun")
    return "wintun";
  if (has_bundled_wintun)
    return "wintun";
  if (!cfg.windows_tap_interface.empty() || !tap_adapters.empty())
    return "tap";
  return "wintun";
}
#endif

nlohmann::json driver_status_json(const Config &cfg) {
  nlohmann::json j{
      {"preferred", cfg.windows_tunnel_driver},
      {"tap_interface", cfg.windows_tap_interface},
      {"supported", false},
  };

#ifdef _WIN32
  std::string wintun_path = utils::get_bundled_wintun_path();
  std::string tap_installer_path = utils::get_bundled_tap_installer_path();
  std::vector<std::string> wintun_adapters = list_windows_adapters("wintun");
  std::vector<std::string> tap_adapters = list_windows_adapters("tap");
  std::string effective =
      effective_windows_driver(cfg, tap_adapters, !wintun_path.empty());

  j["supported"] = true;
  j["effective_driver"] = effective;
  j["wintun_bundled"] = !wintun_path.empty();
  j["wintun_path"] = wintun_path;
  j["wintun_adapters"] = wintun_adapters;
  j["wintun_missing"] = wintun_path.empty();
  j["tap_installer_path"] = tap_installer_path;
  j["tap_can_install"] = !tap_installer_path.empty();
  j["tap_adapters"] = tap_adapters;
  j["tap_available"] = !tap_adapters.empty();
  j["tap_missing"] = !tap_adapters.empty() ? false : tap_installer_path.empty();

  // Wintun missing details
  if (wintun_path.empty()) {
    j["wintun_missing_reason"] = "bundled wintun.dll not found";
    j["wintun_recommended_action"] =
        "Reinstall the desktop package to restore bundled wintun.dll.";
  }

  // TAP missing details
  if (!j.value("tap_available", false) && tap_installer_path.empty()) {
    j["tap_missing_reason"] = "no TAP adapters detected and no bundled installer";
    j["tap_recommended_action"] =
        "Install TAP driver from Settings, or switch tunnel driver to Wintun.";
  } else if (!j.value("tap_available", false)) {
    j["tap_missing_reason"] = "no TAP adapters detected";
    j["tap_recommended_action"] =
        "Click 'Install TAP' to install the TAP adapter.";
  }

  // Effective driver status summary
  bool wintun_ok = !wintun_path.empty();
  bool tap_ok = !tap_adapters.empty();
  if (wintun_ok || tap_ok) {
    // "ready" if the effective/preferred driver works; "degraded" if
    // the preferred driver is missing but a fallback is available.
    bool preferred_ok = false;
    if (cfg.windows_tunnel_driver == "wintun")
      preferred_ok = wintun_ok;
    else if (cfg.windows_tunnel_driver == "tap")
      preferred_ok = tap_ok;
    else // "auto" — prefers wintun
      preferred_ok = wintun_ok;
    j["effective_driver_status"] = preferred_ok ? "ready" : "degraded";
  } else {
    j["effective_driver_status"] = "unavailable";
  }
#endif

  return j;
}

nlohmann::json install_driver(const Config &cfg, const nlohmann::json &payload) {
#ifdef _WIN32
  std::string driver = payload.value("driver", std::string());
  if (driver == "wintun") {
    std::string wintun_path = utils::get_bundled_wintun_path();
    if (wintun_path.empty())
      return structured_error(kErrorRuntimeMissing,
                              "Bundled wintun.dll not found. Add it to the packaged runtime first.",
                              /*recoverable=*/true,
                              "Rebuild the desktop package with the bundled native runtime assets");
    return nlohmann::json{{"ok", true},
                          {"message", "Wintun is bundled and will be activated on the next connection attempt."},
                          {"takes_effect", "next_connect"},
                          {"status", driver_status_json(cfg)}};
  }

  if (driver == "tap") {
    std::string installer = utils::get_bundled_tap_installer_path();
    if (installer.empty()) {
      return structured_error(kErrorRuntimeMissing,
                              "Bundled TAP installer assets are missing. Provide an installer executable or OemVista.inf in the runtime directory.",
                              /*recoverable=*/true,
                              "Rebuild the desktop package with the bundled TAP installer assets");
    }

    int rc = 0;
    if (installer.size() >= 4 &&
        installer.substr(installer.size() - 4) == ".inf") {
      rc = utils::run_command("pnputil /add-driver " + utils::shell_quote(installer) +
                              " /install");
    } else {
      rc = utils::run_command(utils::shell_quote(installer) + " /S");
    }
    if (rc != 0) {
      return structured_error(kErrorNativeFailure,
                              "TAP driver installation failed. Check the bundled installer assets and elevated permissions.",
                              /*recoverable=*/true,
                              "Retry with elevated permissions or check the bundled installer assets");
    }

    return nlohmann::json{{"ok", true},
                          {"message", "TAP driver installation completed."},
                          {"takes_effect", "immediately"},
                          {"status", driver_status_json(cfg)}};
  }

  return structured_error(kErrorUnknownAction,
                          "Unknown driver install target: " + driver,
                          /*recoverable=*/false,
                          "");
#else
  (void)cfg;
  (void)payload;
  return structured_error(kErrorNativeFailure,
                          "Driver installation is only supported on Windows.",
                          /*recoverable=*/false,
                          "");
#endif
}

nlohmann::json preflight_connect(const Config &cfg, const std::string &password) {
  if (cfg.server.empty())
    return structured_error(kErrorConfigInvalid,
                            "VPN server is not configured",
                            /*recoverable=*/true,
                            "Configure the VPN server address in Settings");
  if (cfg.username.empty())
    return structured_error(kErrorConfigInvalid,
                            "VPN username is not configured",
                            /*recoverable=*/true,
                            "Configure your VPN username in Settings");
  if (password.empty())
    return structured_error(kErrorConfigInvalid,
                            "VPN password is not configured",
                            /*recoverable=*/true,
                            "Configure your VPN password in Settings");

  if (!helper::is_available()) {
#ifdef _WIN32
    return structured_error(kErrorServiceMissing,
                            "Helper service is not available. Use the desktop app's Service page to install it with one click, or run 'exv service install' as Administrator.",
                            /*recoverable=*/true,
                            "Install the helper service from the desktop app or CLI");
#else
    return structured_error(kErrorServiceMissing,
                            "Helper daemon is not available. Install the helper service before starting the desktop client.",
                            /*recoverable=*/true,
                            "Install the helper service");
#endif
  }

  nlohmann::json runtime = runtime_status_json(cfg);
  if (!runtime.value("available", false)) {
    return structured_error(kErrorRuntimeMissing,
                            "OpenConnect runtime is not available",
                            /*recoverable=*/true,
                            "Ensure the desktop package contains the bundled OpenConnect runtime");
  }

#ifdef _WIN32
  nlohmann::json drivers = driver_status_json(cfg);
  std::string effective = drivers.value("effective_driver", std::string("wintun"));
  if (cfg.windows_tunnel_driver == "wintun" &&
      !drivers.value("wintun_bundled", false)) {
    return structured_error(kErrorRuntimeMissing,
                            "Wintun is selected but bundled wintun.dll is missing",
                            /*recoverable=*/true,
                            "Rebuild the desktop package with the bundled native runtime assets");
  }
  if (effective == "tap" && cfg.windows_tunnel_driver == "tap" &&
      cfg.windows_tap_interface.empty()) {
    return structured_error(kErrorConfigInvalid,
                            "TAP is selected but no TAP interface is configured",
                            /*recoverable=*/true,
                            "Choose an installed TAP adapter from the Drivers page, or switch back to Wintun in Settings");
  }
#endif

  return nlohmann::json{{"ok", true}};
}

nlohmann::json logs_json(const nlohmann::json &payload) {
  config::ConfigManager mgr = make_config_manager();
  Config cfg = mgr.load();
  std::string log_path = utils::expand_home(cfg.log_file);
  int max_lines = payload.value("lines", 100);
  if (max_lines < 1)
    max_lines = 1;
  if (max_lines > 10000)
    max_lines = 10000;
  std::string filter = payload.value("filter", std::string());

  nlohmann::json lines = nlohmann::json::array();
  std::vector<std::string> all_lines;
  std::ifstream ifs(log_path);
  std::string line;
  while (std::getline(ifs, line)) {
    if (!line.empty() && line.back() == '\r')
      line.pop_back();
    if (filter.empty() || line.find(filter) != std::string::npos)
      all_lines.push_back(line);
  }

  size_t start = all_lines.size() > static_cast<size_t>(max_lines)
                     ? all_lines.size() - static_cast<size_t>(max_lines)
                     : 0;
  for (size_t i = start; i < all_lines.size(); ++i) {
    lines.push_back({{"timestamp", ""},
                     {"level", "info"},
                     {"message", all_lines[i]}});
  }
  return lines;
}

} // namespace

nlohmann::json handle_action(const std::string &action,
                             const nlohmann::json &payload) {
  try {
    config::ConfigManager mgr = make_config_manager();
    Config cfg = mgr.load();

    // --- status.get: recommended status query (prefers helper) ---
    if (action == "status.get") {
      if (helper::is_available()) {
        auto helper_resp = send_helper_request({{"action", "status"}});
        if (helper_resp.value("ok", false)) {
          auto j = frontend_status_from_helper(helper_resp, cfg);
          j["mode"] = "helper";
          return j;
        }
        // Helper responded but with error — if it's not a "helper not available"
        // error, still try to return status from the helper response
        if (helper_resp.value("error_type", "") != kErrorServiceMissing) {
          auto j = frontend_status_from_helper(helper_resp, cfg);
          j["mode"] = "helper";
          return j;
        }
      }
      // Helper not available — check direct VPN state
      auto j = vpn::direct_status_json(cfg);
      // direct_status_json already sets mode to "direct" or "disconnected"
      return j;
    }

    // --- vpn.disconnect: tries helper first, falls back to direct ---
    if (action == "vpn.disconnect") {
      if (helper::is_available()) {
        auto helper_resp = send_helper_request({{"action", "stop"}});
        if (helper_resp.value("ok", false)) {
          auto j = disconnected_status(cfg);
          j["mode"] = "helper";
          return j;
        }
        // Helper failed but is available — return the error, don't fallback
        return structured_error(kErrorNativeFailure,
                                helper_resp.value("message", "Failed to stop VPN"),
                                /*recoverable=*/true,
                                "Retry the operation");
      }

      bool allow_direct = payload.value("allow_direct_fallback", false);
      if (allow_direct) {
        auto result = vpn::direct_stop_json();
        if (result.value("ok", false)) {
          result["mode"] = "direct";
          result["connected"] = false;
          result["network_ready"] = false;
        }
        return result;
      }

      return structured_error(kErrorServiceMissing,
#ifdef _WIN32
                              "Helper service is not installed. Install the service or use elevated mode.",
#else
                              "Helper daemon is not available. Install the service or use elevated mode.",
#endif
                              /*recoverable=*/true,
                              "Install the helper service or use elevated mode");
    }

    // --- Direct-mode actions: bypass the helper daemon ---
    // These require elevated privileges and are used by the Electron shell
    // when the helper service is not installed.

    if (action == "status.get.direct") {
      return vpn::direct_status_json(cfg);
    }

    if (action == "vpn.connect.direct") {
      std::string password = payload.value("password", std::string());
      if (password.empty() && !cfg.password.empty()) {
        std::string key = crypto::load_key();
        if (!key.empty())
          password = crypto::decrypt(cfg.password, key);
      }
      int retry_limit = payload.value("retry_limit", 0);
      return vpn::direct_start_json(cfg, password, retry_limit);
    }

    if (action == "vpn.disconnect.direct") {
      auto result = vpn::direct_stop_json();
      // Tag mode on success responses
      if (result.value("ok", false)) {
        result["mode"] = "direct";
        result["connected"] = false;
        result["network_ready"] = false;
      }
      return result;
    }

    // --- vpn.connect: tries helper first, falls back to direct ---
    if (action == "vpn.connect") {
      std::string password = payload.value("password", std::string());
      if (password.empty() && !cfg.password.empty()) {
        std::string key = crypto::load_key();
        if (!key.empty())
          password = crypto::decrypt(cfg.password, key);
      }

      bool allow_direct = payload.value("allow_direct_fallback", false);

      // Try helper first
      if (helper::is_available()) {
        nlohmann::json preflight = preflight_connect(cfg, password);
        if (preflight.is_object() && preflight.value("ok", true) == false)
          return preflight;
        auto helper_resp = send_helper_request({{"action", "start"},
                                                {"config", cfg},
                                                {"password", password},
                                                {"retry_limit", 0},
                                                {"home", utils::get_effective_home()},
                                                {"config_dir", utils::get_config_dir()}});
        if (helper_resp.value("ok", false)) {
          auto j = frontend_status_from_helper(helper_resp, cfg);
          j["mode"] = "helper";
          return j;
        }
        // Helper failed but is available — return the error, don't fallback
        return structured_error(kErrorNativeFailure,
                                helper_resp.value("message", "Failed to start VPN"),
                                /*recoverable=*/true,
                                "Retry the operation");
      }

      // Helper not available
      if (allow_direct) {
        return vpn::direct_start_json(cfg, password, 0);
      }

      return structured_error(kErrorServiceMissing,
#ifdef _WIN32
                              "Helper service is not installed. Install the service or use elevated mode.",
#else
                              "Helper daemon is not available. Install the service or use elevated mode.",
#endif
                              /*recoverable=*/true,
                              "Install the helper service or use elevated mode");
    }

    if (action == "config.getAuth")
      return auth_config(cfg);

    if (action == "config.saveAuth") {
      if (payload.contains("server") && payload["server"].is_string()) {
        std::string err = config_api::config_set(mgr, "server", payload["server"].get<std::string>());
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      if (payload.contains("username") && payload["username"].is_string()) {
        std::string err = config_api::config_set(mgr, "username", payload["username"].get<std::string>());
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      // Update remember_password BEFORE password so that the password setter
      // sees the correct toggle state. The UI treats an empty password field
      // as "keep the existing password" while a non-empty value means update.
      if (payload.contains("remember_password") &&
          payload["remember_password"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "remember_password",
                               payload["remember_password"].get<bool>() ? "true"
                                                                         : "false");
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      if (payload.contains("password") && payload["password"].is_string()) {
        std::string password = payload["password"].get<std::string>();
        if (!password.empty()) {
          std::string err = config_api::config_set_password(mgr, password);
          if (!err.empty())
            return structured_error(kErrorConfigInvalid, err);
        }
      }
      if (payload.contains("user_agent") && payload["user_agent"].is_string()) {
        std::string value = payload["user_agent"].get<std::string>();
        // Treat empty / whitespace user_agent as "no change" so the UI cannot
        // accidentally overwrite the platform default.
        if (!utils::trim(value).empty()) {
          std::string err = config_api::config_set(mgr, "useragent", value);
          if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
        }
      }
      return auth_config(mgr.load());
    }

    if (action == "config.getSettings")
      return settings_config(cfg);

    if (action == "config.saveSettings") {
      if (payload.contains("mtu") && payload["mtu"].is_number_integer()) {
        std::string err = config_api::config_set(mgr, "mtu", std::to_string(payload["mtu"].get<int>()));
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      if (payload.contains("dtls") && payload["dtls"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "disable_dtls",
                               payload["dtls"].get<bool>() ? "false" : "true");
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      if (payload.contains("extra_args") && payload["extra_args"].is_string()) {
        Config updated = mgr.load();
        std::string value = payload["extra_args"].get<std::string>();
        updated.extra_args = value.empty() ? std::vector<std::string>{}
                                           : std::vector<std::string>{value};
        mgr.save(updated);
      }
      if (payload.contains("log_path") && payload["log_path"].is_string()) {
        std::string err = config_api::config_set(mgr, "log_file", payload["log_path"].get<std::string>());
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      if (payload.contains("webui_port") && payload["webui_port"].is_number_integer()) {
        Config updated = mgr.load();
        updated.webui_port = payload["webui_port"].get<int>();
        mgr.save(updated);
      }
      if (payload.contains("webui_host") && payload["webui_host"].is_string()) {
        Config updated = mgr.load();
        updated.webui_bind = payload["webui_host"].get<std::string>();
        mgr.save(updated);
      }
      if (payload.contains("webui_enabled") && payload["webui_enabled"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "webui_enabled",
                               payload["webui_enabled"].get<bool>() ? "true" : "false");
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      if (payload.contains("openconnect_runtime") &&
          payload["openconnect_runtime"].is_string()) {
        std::string err = config_api::config_set(mgr, "openconnect_runtime",
                               payload["openconnect_runtime"].get<std::string>());
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      if (payload.contains("windows_tunnel_driver") &&
          payload["windows_tunnel_driver"].is_string()) {
        std::string err = config_api::config_set(mgr, "windows_tunnel_driver",
                               payload["windows_tunnel_driver"].get<std::string>());
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      if (payload.contains("windows_tap_interface") &&
          payload["windows_tap_interface"].is_string()) {
        std::string err = config_api::config_set(mgr, "windows_tap_interface",
                               payload["windows_tap_interface"].get<std::string>());
        if (!err.empty()) return structured_error(kErrorConfigInvalid, err);
      }
      return settings_config(mgr.load());
    }

    if (action == "config.getKey")
      return key_status_json();

    if (action == "routes.list")
      return routes_json(cfg);

    if (action == "routes.add") {
      std::string err = config_api::route_add(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return structured_error(kErrorConfigInvalid, err);
      return routes_json(mgr.load());
    }

    if (action == "routes.remove") {
      std::string err = config_api::route_remove(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return structured_error(kErrorConfigInvalid, err);
      return routes_json(mgr.load());
    }

    if (action == "routes.reset") {
      config_api::route_reset_defaults(mgr);
      return routes_json(mgr.load());
    }

    if (action == "service.status")
      return service_status_json();

    if (action == "runtime.status")
      return runtime_status_json(cfg);

    if (action == "drivers.status")
      return driver_status_json(cfg);

    if (action == "drivers.install")
      return install_driver(cfg, payload);

    if (action == "logs.list")
      return logs_json(payload);

    return structured_error(kErrorUnknownAction,
                            "Unknown desktop action: " + action,
                            /*recoverable=*/false,
                            "");
  } catch (const std::exception &ex) {
    return structured_error(kErrorNativeFailure,
                            ex.what(),
                            /*recoverable=*/true,
                            "Retry the operation");
  } catch (...) {
    return structured_error(kErrorNativeFailure,
                            "Unknown desktop API error",
                            /*recoverable=*/true,
                            "Retry the operation");
  }
}

} // namespace app_api
} // namespace ecnuvpn
