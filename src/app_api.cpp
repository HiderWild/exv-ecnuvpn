#include "app_api.hpp"

#include "config.hpp"
#include "config_api.hpp"
#include "config_manager.hpp"
#include "crypto.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "utils.hpp"
#include "vpn.hpp"
#include "virtual_network.hpp"

#include <algorithm>
#include <cstdio>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#ifndef _WIN32
#include <cerrno>
#include <signal.h>
#include <sys/socket.h>
#include <sys/stat.h>
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

nlohmann::json error(const std::string &message) {
  return nlohmann::json{{"ok", false}, {"error", message}};
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
    return nlohmann::json{{"ok", false},
                          {"message", "Helper daemon not available"}};
  }

  DWORD bytesWritten = 0;
  if (!WriteFile(hPipe, payload.c_str(), static_cast<DWORD>(payload.size()),
                 &bytesWritten, NULL) ||
      bytesWritten != payload.size()) {
    CloseHandle(hPipe);
    return nlohmann::json{{"ok", false},
                          {"message", "Failed to send helper request"}};
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
    return nlohmann::json{{"ok", false}, {"message", "Failed to create socket"}};
  }

  sockaddr_un addr {};
  addr.sun_family = AF_UNIX;
  std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", kHelperSocketPath);

  if (connect(fd, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) != 0) {
    close(fd);
    return nlohmann::json{{"ok", false},
                          {"message", "Helper daemon not available"}};
  }

  if (write(fd, payload.data(), payload.size()) !=
      static_cast<ssize_t>(payload.size())) {
    close(fd);
    return nlohmann::json{{"ok", false},
                          {"message", "Failed to send helper request"}};
  }
  shutdown(fd, SHUT_WR);

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
    return nlohmann::json{{"ok", false}, {"message", "Empty helper response"}};
  }

  try {
    return nlohmann::json::parse(raw);
  } catch (...) {
    return nlohmann::json{{"ok", false},
                          {"message", "Failed to parse helper response"}};
  }
}

nlohmann::json frontend_status_from_helper(const nlohmann::json &helper_resp,
                                           const Config &cfg) {
  nlohmann::json j;
  bool running = helper_resp.value("running", false);
  j["connected"] = running;
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

  // Session mode: helper-connected sessions are "helper" mode
  j["session_mode"] = running ? "helper" : "disconnected";
  j["cleanup_pending"] = false;

  return j;
}

nlohmann::json disconnected_status(const Config &cfg) {
  auto j = frontend_status_from_helper(nlohmann::json{{"running", false}}, cfg);
  j["session_mode"] = "disconnected";
  j["cleanup_pending"] = false;
  return j;
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

  nlohmann::json j{
      {"mtu", cfg.mtu},
      {"dtls", !cfg.disable_dtls},
      {"extra_args", extra_args},
      {"log_path", cfg.log_file},
      {"webui_port", cfg.webui_port},
      {"webui_host", cfg.webui_bind},
      {"webui_enabled", cfg.webui_enabled},
      {"openconnect_runtime", cfg.openconnect_runtime},
  };

#ifdef _WIN32
  j["windows_tunnel_driver"] = cfg.windows_tunnel_driver;
  j["windows_tap_interface"] = cfg.windows_tap_interface;
#endif

  return j;
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

nlohmann::json helper_status_json() {
  bool available = helper::is_available();
  nlohmann::json j;
#ifdef __APPLE__
  j["installed"] =
      utils::file_exists("/Library/LaunchDaemons/com.ecnu.exv.helper.plist");
  j["socket_path"] = "/var/run/exv-helper.sock";
  j["label"] = "com.ecnu.exv.helper";
#elif defined(__linux__)
  j["installed"] = utils::file_exists("/etc/systemd/system/exv-helper.service");
  j["socket_path"] = "/var/run/exv-helper.sock";
  j["label"] = "exv-helper";
#elif defined(_WIN32)
  j["socket_path"] = "\\\\.\pipe\\exv-helper";
  j["label"] = "exv-helper";
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
  j["wintun_path"] = utils::get_bundled_wintun_path();
  j["tap_installer_path"] = utils::get_bundled_tap_installer_path();
#endif
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
  j["tap_installer_path"] = tap_installer_path;
  j["tap_can_install"] = !tap_installer_path.empty();
  j["tap_adapters"] = tap_adapters;
  j["tap_available"] = !tap_adapters.empty();
#endif

  return j;
}

nlohmann::json install_driver(const Config &cfg, const nlohmann::json &payload) {
#ifdef _WIN32
  std::string driver = payload.value("driver", std::string());
  if (driver == "wintun") {
    std::string wintun_path = utils::get_bundled_wintun_path();
    if (wintun_path.empty())
      return error("Bundled wintun.dll not found. Add it to the packaged runtime first.");
    return nlohmann::json{{"ok", true},
                          {"message", "Wintun is bundled and will be activated on the next connection attempt."},
                          {"status", driver_status_json(cfg)}};
  }

  if (driver == "tap") {
    std::string installer = utils::get_bundled_tap_installer_path();
    if (installer.empty()) {
      return error("Bundled TAP installer assets are missing. Provide an installer executable or OemVista.inf in the runtime directory.");
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
      return error("TAP driver installation failed. Check the bundled installer assets and elevated permissions.");
    }

    return nlohmann::json{{"ok", true},
                          {"message", "TAP driver installation completed."},
                          {"status", driver_status_json(cfg)}};
  }

  return error("Unknown driver install target: " + driver);
#else
  (void)cfg;
  (void)payload;
  return error("Driver installation is only supported on Windows.");
#endif
}

nlohmann::json preflight_connect(const Config &cfg, const std::string &password) {
  if (cfg.server.empty())
    return error("VPN server is not configured.");
  if (cfg.username.empty())
    return error("VPN username is not configured.");
  if (password.empty())
    return error("VPN password is not configured.");

  nlohmann::json runtime = runtime_status_json(cfg);
  if (!runtime.value("available", false)) {
    return error("OpenConnect runtime is not available. The desktop bundle is missing openconnect and its native dependencies.");
  }

  if (!helper::is_available()) {
#ifdef __APPLE__
    // On macOS, the Electron main process can use osascript to run
    // vpn.connect with administrator privileges as a fallback.
    return nlohmann::json{{"ok", false},
                          {"error", "helper_missing"},
                          {"message", "Helper daemon is not available. The desktop app can connect via one-time administrator authorization, or install the helper service for persistent connections."}};
#elif defined(_WIN32)
    return error("Helper daemon is not available. Install the helper service from Settings or run 'exv service install' as Administrator.");
#else
    return error("Helper daemon is not available. Install the helper service before starting the desktop client.");
#endif
  }

#ifdef _WIN32
  nlohmann::json drivers = driver_status_json(cfg);
  std::string effective = drivers.value("effective_driver", std::string("wintun"));
  if (cfg.windows_tunnel_driver == "wintun" &&
      !drivers.value("wintun_bundled", false)) {
    return error("Wintun is selected but bundled wintun.dll is missing.");
  }
  if (effective == "tap" && cfg.windows_tunnel_driver == "tap" &&
      cfg.windows_tap_interface.empty()) {
    return error("TAP is selected but no TAP interface is configured. Choose an installed TAP adapter or switch back to Wintun.");
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

    if (action == "status.get") {
      auto helper_resp = send_helper_request({{"action", "status"}});
      if (!helper_resp.value("ok", false) &&
          helper_resp.value("message", "") == "Helper daemon not available") {
        // Helper unavailable — check if an elevated direct-mode VPN is running
        // by probing the PID file and route-ready marker.
        std::string pid_str = utils::trim(utils::read_file(utils::get_pid_path()));
        pid_t vpn_pid = -1;
        try { vpn_pid = std::stoi(pid_str); } catch (...) {}
        bool alive = false;
        if (vpn_pid > 0) {
#ifndef _WIN32
          alive = (kill(vpn_pid, 0) == 0 || errno == EPERM);
#else
          HANDLE h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE,
                                 static_cast<DWORD>(vpn_pid));
          if (h) {
            DWORD exitCode = 0;
            alive = GetExitCodeProcess(h, &exitCode) && exitCode == STILL_ACTIVE;
            CloseHandle(h);
          }
#endif
        }
        if (alive) {
          std::string ready_path = utils::get_route_ready_path();
          std::string iface, ip;
          if (utils::file_exists(ready_path)) {
            std::istringstream iss(utils::read_file(ready_path));
            std::string line;
            if (std::getline(iss, line)) iface = utils::trim(line);
            if (std::getline(iss, line)) ip = utils::trim(line);
          }
          nlohmann::json j = disconnected_status(cfg);
          j["connected"] = true;
          j["pid"] = vpn_pid;
          j["interface"] = iface;
          j["internal_ip"] = ip;
          j["network_ready"] = !iface.empty();
          j["session_mode"] = "elevated";
          virtual_network::add_status_fields(j, iface);
          return j;
        }
        return disconnected_status(cfg);
      }
      return frontend_status_from_helper(helper_resp, cfg);
    }

    if (action == "vpn.connect") {
      std::string password = payload.value("password", std::string());
      if (password.empty() && !cfg.password.empty()) {
        std::string key = crypto::load_key();
        if (!key.empty())
          password = crypto::decrypt(cfg.password, key);
      }

      // When running as root (elevated via osascript), bypass the helper
      // and connect directly — the helper is not needed with root privileges.
      if (!helper::is_available() && utils::check_root()) {
        if (cfg.server.empty())
          return error("VPN server is not configured.");
        if (cfg.username.empty())
          return error("VPN username is not configured.");
        if (password.empty())
          return error("VPN password is not configured.");
        nlohmann::json runtime = runtime_status_json(cfg);
        if (!runtime.value("available", false))
          return error("OpenConnect runtime is not available. The desktop bundle is missing openconnect and its native dependencies.");

        // Set runtime owner so files written as root are chowned to the
        // real user (derived from the home directory's owner).
        std::string home = utils::get_effective_home();
        if (!home.empty()) {
          struct stat home_st;
          if (stat(home.c_str(), &home_st) == 0) {
            utils::set_runtime_owner(home_st.st_uid, home_st.st_gid);
            utils::set_runtime_path_override(home, utils::get_config_dir());
          }
        }
        utils::fix_config_dir_ownership();

        int rc = vpn::start_with_password(cfg, password, 0);
        if (rc != 0)
          return error("Failed to start VPN (elevated direct mode).");

        nlohmann::json result = disconnected_status(cfg);
        result["connected"] = true;
        result["session_mode"] = "elevated";
        return result;
      }

      nlohmann::json preflight = preflight_connect(cfg, password);
      if (preflight.is_object() && preflight.value("ok", true) == false)
        return preflight;
      auto helper_resp = send_helper_request({{"action", "start"},
                                              {"config", cfg},
                                              {"password", password},
                                              {"retry_limit", 0},
                                              {"home", utils::get_effective_home()},
                                              {"config_dir", utils::get_config_dir()}});
      if (!helper_resp.value("ok", false)) {
        return error(helper_resp.value("message", "Failed to start VPN"));
      }
      return frontend_status_from_helper(helper_resp, cfg);
    }

    if (action == "vpn.disconnect") {
      auto helper_resp = send_helper_request({{"action", "stop"}});
      if (!helper_resp.value("ok", false)) {
        return error(helper_resp.value("message", "Failed to stop VPN"));
      }
      return disconnected_status(cfg);
    }

    if (action == "config.getAuth")
      return auth_config(cfg);

    if (action == "config.saveAuth") {
      if (payload.contains("server") && payload["server"].is_string()) {
        std::string err = config_api::config_set(mgr, "server", payload["server"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("username") && payload["username"].is_string()) {
        std::string err = config_api::config_set(mgr, "username", payload["username"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      // Update remember_password BEFORE password so that the password setter
      // sees the correct toggle state. The UI treats an empty password field
      // as "keep the existing password" while a non-empty value means update.
      if (payload.contains("remember_password") &&
          payload["remember_password"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "remember_password",
                               payload["remember_password"].get<bool>() ? "true"
                                                                         : "false");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("password") && payload["password"].is_string()) {
        std::string password = payload["password"].get<std::string>();
        if (!password.empty()) {
          std::string err = config_api::config_set_password(mgr, password);
          if (!err.empty())
            return error(err);
        }
      }
      if (payload.contains("user_agent") && payload["user_agent"].is_string()) {
        std::string value = payload["user_agent"].get<std::string>();
        // Treat empty / whitespace user_agent as "no change" so the UI cannot
        // accidentally overwrite the platform default.
        if (!utils::trim(value).empty()) {
          std::string err = config_api::config_set(mgr, "useragent", value);
          if (!err.empty()) return error(err);
        }
      }
      return auth_config(mgr.load());
    }

    if (action == "config.getSettings")
      return settings_config(cfg);

    if (action == "config.saveSettings") {
      if (payload.contains("mtu") && payload["mtu"].is_number_integer()) {
        std::string err = config_api::config_set(mgr, "mtu", std::to_string(payload["mtu"].get<int>()));
        if (!err.empty()) return error(err);
      }
      if (payload.contains("dtls") && payload["dtls"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "disable_dtls",
                               payload["dtls"].get<bool>() ? "false" : "true");
        if (!err.empty()) return error(err);
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
        if (!err.empty()) return error(err);
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
        if (!err.empty()) return error(err);
      }
      if (payload.contains("openconnect_runtime") &&
          payload["openconnect_runtime"].is_string()) {
        std::string err = config_api::config_set(mgr, "openconnect_runtime",
                               payload["openconnect_runtime"].get<std::string>());
        if (!err.empty()) return error(err);
      }
#ifdef _WIN32
      if (payload.contains("windows_tunnel_driver") &&
          payload["windows_tunnel_driver"].is_string()) {
        std::string err = config_api::config_set(mgr, "windows_tunnel_driver",
                               payload["windows_tunnel_driver"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("windows_tap_interface") &&
          payload["windows_tap_interface"].is_string()) {
        std::string err = config_api::config_set(mgr, "windows_tap_interface",
                               payload["windows_tap_interface"].get<std::string>());
        if (!err.empty()) return error(err);
      }
#endif
      return settings_config(mgr.load());
    }

    if (action == "config.getKey")
      return key_status_json();

    if (action == "routes.list")
      return routes_json(cfg);

    if (action == "routes.add") {
      std::string err = config_api::route_add(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return error(err);
      return routes_json(mgr.load());
    }

    if (action == "routes.remove") {
      std::string err = config_api::route_remove(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return error(err);
      return routes_json(mgr.load());
    }

    if (action == "routes.reset") {
      config_api::route_reset_defaults(mgr);
      return routes_json(mgr.load());
    }

    if (action == "service.status")
      return helper_status_json();

    if (action == "helper.status")
      return helper_status_json();

    if (action == "runtime.status")
      return runtime_status_json(cfg);

    if (action == "logs.list")
      return logs_json(payload);

    return error("Unknown desktop action: " + action);
  } catch (const std::exception &ex) {
    return error(ex.what());
  } catch (...) {
    return error("Unknown desktop API error");
  }
}

} // namespace app_api
} // namespace ecnuvpn
