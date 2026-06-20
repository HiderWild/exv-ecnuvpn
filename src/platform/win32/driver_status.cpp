#include "utils/strings.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/driver_status.hpp"

#include <chrono>
#include <functional>
#include <mutex>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace exv {
namespace platform {
namespace {

using Clock = std::chrono::steady_clock;

std::mutex g_snapshot_mutex;
std::optional<WindowsDriverAdapterSnapshot> g_cached_snapshot;
Clock::time_point g_cached_snapshot_at{};
std::function<WindowsDriverAdapterSnapshot()> g_snapshot_provider_for_testing;
constexpr auto kDriverSnapshotCacheTtl = std::chrono::seconds(2);

void clear_driver_status_cache_locked() {
  g_cached_snapshot.reset();
  g_cached_snapshot_at = Clock::time_point{};
}

void add_adapter_line(WindowsDriverAdapterSnapshot *snapshot,
                      const std::string &line) {
  if (!snapshot || line.empty()) {
    return;
  }

  const std::string wintun_prefix = "wintun\t";
  const std::string tap_prefix = "tap\t";
  if (line.rfind(wintun_prefix, 0) == 0) {
    snapshot->wintun_adapters.push_back(line.substr(wintun_prefix.size()));
  } else if (line.rfind(tap_prefix, 0) == 0) {
    snapshot->tap_adapters.push_back(line.substr(tap_prefix.size()));
  }
}

WindowsDriverAdapterSnapshot query_windows_adapter_snapshot() {
  std::string command =
      "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "
      "\"Get-CimInstance Win32_NetworkAdapter | Where-Object { "
      "$_.NetEnabled -ne $false } | ForEach-Object { "
      "$name = if ($_.NetConnectionID) { $_.NetConnectionID } else { $_.Name }; "
      "$kind = ''; "
      "if ($_.NetConnectionID -like '*Wintun*' -or $_.Name -like '*Wintun*' -or $_.Description -like '*Wintun*') { $kind = 'wintun' } "
      "elseif ($_.NetConnectionID -like '*TAP*' -or $_.Name -like '*TAP*' -or $_.Description -like '*TAP-Windows*' -or $_.Description -like '*tap0901*') { $kind = 'tap' }; "
      "if ($kind -and $name) { $kind + [char]9 + $name } }\"";

  WindowsDriverAdapterSnapshot snapshot;
  for (const auto &line : exv::utils::split_lines(platform::run_command_output(command))) {
    add_adapter_line(&snapshot, line);
  }
  return snapshot;
}

WindowsDriverAdapterSnapshot adapter_snapshot() {
  const auto now = Clock::now();
  {
    std::lock_guard<std::mutex> lock(g_snapshot_mutex);
    if (g_cached_snapshot &&
        now - g_cached_snapshot_at <= kDriverSnapshotCacheTtl) {
      return *g_cached_snapshot;
    }
  }

  WindowsDriverAdapterSnapshot snapshot;
  std::function<WindowsDriverAdapterSnapshot()> provider;
  {
    std::lock_guard<std::mutex> lock(g_snapshot_mutex);
    provider = g_snapshot_provider_for_testing;
  }
  snapshot = provider ? provider() : query_windows_adapter_snapshot();

  {
    std::lock_guard<std::mutex> lock(g_snapshot_mutex);
    g_cached_snapshot = snapshot;
    g_cached_snapshot_at = now;
  }
  return snapshot;
}

std::string effective_windows_driver(const ConfigView &cfg,
                                     const std::vector<std::string> &tap_adapters,
                                     bool wintun_available) {
  if (cfg.windows_tunnel_driver == "tap")
    return "tap";
  if (cfg.windows_tunnel_driver == "wintun")
    return "wintun";
  if (wintun_available)
    return "wintun";
  if (!cfg.windows_tap_interface.empty() || !tap_adapters.empty())
    return "tap";
  return "wintun";
}

std::string effective_driver_status(const ConfigView &cfg,
                                    const std::string &effective,
                                    bool wintun_available,
                                    bool tap_available) {
  bool effective_available =
      (effective == "wintun" && wintun_available) ||
      (effective == "tap" && tap_available);
  if (effective_available)
    return "ready";

  if (cfg.windows_tunnel_driver == "auto" && (wintun_available || tap_available))
    return "degraded";

  return "unavailable";
}

} // namespace

void set_driver_status_adapter_snapshot_provider_for_testing(
    std::function<WindowsDriverAdapterSnapshot()> provider) {
  std::lock_guard<std::mutex> lock(g_snapshot_mutex);
  g_snapshot_provider_for_testing = std::move(provider);
  clear_driver_status_cache_locked();
}

void invalidate_driver_status_cache() {
  std::lock_guard<std::mutex> lock(g_snapshot_mutex);
  clear_driver_status_cache_locked();
}

void clear_driver_status_cache_for_testing() {
  invalidate_driver_status_cache();
}

nlohmann::json driver_status_json(const ConfigView &cfg) {
  nlohmann::json json{{"preferred", cfg.windows_tunnel_driver},
                      {"tap_interface", cfg.windows_tap_interface},
                      {"supported", true}};

  std::string wintun_path = platform::get_bundled_wintun_path();
  std::string tap_installer_path = platform::get_bundled_tap_installer_path();
  WindowsDriverAdapterSnapshot adapters = adapter_snapshot();
  std::vector<std::string> wintun_adapters = adapters.wintun_adapters;
  std::vector<std::string> tap_adapters = adapters.tap_adapters;
  bool wintun_bundled = !wintun_path.empty();
  bool wintun_available = wintun_bundled;
  bool tap_available = !tap_adapters.empty();
  std::string effective =
      effective_windows_driver(cfg, tap_adapters, wintun_available);

  json["effective_driver"] = effective;
  json["effective_driver_status"] =
      effective_driver_status(cfg, effective, wintun_available, tap_available);
  json["wintun_bundled"] = wintun_bundled;
  json["wintun_path"] = wintun_path;
  json["wintun_adapters"] = wintun_adapters;
  json["wintun_adapter_present"] = !wintun_adapters.empty();
  json["wintun_missing"] = !wintun_available;
  json["wintun_missing_reason"] =
      wintun_available
          ? ""
          : (!wintun_adapters.empty()
                 ? "Existing Wintun adapters were detected, but the packaged wintun.dll runtime is missing."
                 : "No packaged wintun.dll runtime was detected.");
  json["wintun_recommended_action"] =
      wintun_available
          ? ""
          : "Stage wintun.dll in the packaged runtime directory.";
  json["tap_installer_path"] = tap_installer_path;
  json["tap_can_install"] = !tap_installer_path.empty();
  json["tap_adapters"] = tap_adapters;
  json["tap_available"] = tap_available;
  json["tap_missing"] = !tap_available;
  json["tap_missing_reason"] =
      tap_available ? "" : "No TAP-Windows adapter was detected.";
  json["tap_recommended_action"] =
      tap_available
          ? ""
          : (!tap_installer_path.empty()
                 ? "Install the bundled TAP driver from Settings."
                 : "Provide TAP installer assets in the runtime directory or use Wintun.");
  return json;
}

nlohmann::json install_driver(const ConfigView &cfg,
                              const nlohmann::json &payload) {
  invalidate_driver_status_cache();
  std::string driver = payload.value("driver", std::string());
  if (driver == "wintun") {
    std::string wintun_path = platform::get_bundled_wintun_path();
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
    std::string installer = platform::get_bundled_tap_installer_path();
    if (installer.empty()) {
      return nlohmann::json{{"ok", false},
                            {"error",
                             "Bundled TAP installer assets are missing. Provide an installer executable or OemVista.inf in the runtime directory."}};
    }

    int rc = 0;
    if (installer.size() >= 4 &&
        installer.substr(installer.size() - 4) == ".inf") {
      rc = platform::run_command("pnputil /add-driver " +
                              platform::shell_quote(installer) + " /install");
    } else {
      rc = platform::run_command(platform::shell_quote(installer) + " /S");
    }
    if (rc != 0) {
      invalidate_driver_status_cache();
      return nlohmann::json{{"ok", false},
                            {"error",
                             "TAP driver installation failed. Check the bundled installer assets and elevated permissions."}};
    }

    invalidate_driver_status_cache();
    return nlohmann::json{{"ok", true},
                          {"message", "TAP driver installation completed."},
                          {"status", driver_status_json(cfg)}};
  }

  return nlohmann::json{{"ok", false},
                        {"error", "Unknown driver install target: " + driver}};
}

} // namespace platform
} // namespace exv
