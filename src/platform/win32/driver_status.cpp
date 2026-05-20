#include "platform/common/driver_status.hpp"

#include "utils.hpp"

#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {
namespace {

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

} // namespace

nlohmann::json driver_status_json(const Config &cfg) {
  nlohmann::json json{{"preferred", cfg.windows_tunnel_driver},
                      {"tap_interface", cfg.windows_tap_interface},
                      {"supported", true}};

  std::string wintun_path = utils::get_bundled_wintun_path();
  std::string tap_installer_path = utils::get_bundled_tap_installer_path();
  std::vector<std::string> wintun_adapters = list_windows_adapters("wintun");
  std::vector<std::string> tap_adapters = list_windows_adapters("tap");
  std::string effective =
      effective_windows_driver(cfg, tap_adapters, !wintun_path.empty());

  json["effective_driver"] = effective;
  json["wintun_bundled"] = !wintun_path.empty();
  json["wintun_path"] = wintun_path;
  json["wintun_adapters"] = wintun_adapters;
  json["tap_installer_path"] = tap_installer_path;
  json["tap_can_install"] = !tap_installer_path.empty();
  json["tap_adapters"] = tap_adapters;
  json["tap_available"] = !tap_adapters.empty();
  return json;
}

nlohmann::json install_driver(const Config &cfg,
                              const nlohmann::json &payload) {
  std::string driver = payload.value("driver", std::string());
  if (driver == "wintun") {
    std::string wintun_path = utils::get_bundled_wintun_path();
    if (wintun_path.empty()) {
      return nlohmann::json{{"ok", false},
                            {"error",
                             "Bundled wintun.dll not found. Add it to the packaged runtime first."}};
    }
    return nlohmann::json{{"ok", true},
                          {"message", "Wintun is bundled and will be activated on the next connection attempt."},
                          {"status", driver_status_json(cfg)}};
  }

  if (driver == "tap") {
    std::string installer = utils::get_bundled_tap_installer_path();
    if (installer.empty()) {
      return nlohmann::json{{"ok", false},
                            {"error",
                             "Bundled TAP installer assets are missing. Provide an installer executable or OemVista.inf in the runtime directory."}};
    }

    int rc = 0;
    if (installer.size() >= 4 &&
        installer.substr(installer.size() - 4) == ".inf") {
      rc = utils::run_command("pnputil /add-driver " +
                              utils::shell_quote(installer) + " /install");
    } else {
      rc = utils::run_command(utils::shell_quote(installer) + " /S");
    }
    if (rc != 0) {
      return nlohmann::json{{"ok", false},
                            {"error",
                             "TAP driver installation failed. Check the bundled installer assets and elevated permissions."}};
    }

    return nlohmann::json{{"ok", true},
                          {"message", "TAP driver installation completed."},
                          {"status", driver_status_json(cfg)}};
  }

  return nlohmann::json{{"ok", false},
                        {"error", "Unknown driver install target: " + driver}};
}

} // namespace platform
} // namespace ecnuvpn