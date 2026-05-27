#include "platform/common/app_api_runtime_policy.hpp"

#include "utils.hpp"
#include "vpn.hpp"

#include <sys/stat.h>

namespace ecnuvpn {
namespace platform {

void prepare_direct_fallback_runtime() {
  if (!utils::check_root())
    return;

  std::string home = utils::get_effective_home();
  if (home.empty())
    return;

  struct stat home_stat {};
  if (stat(home.c_str(), &home_stat) == 0) {
    utils::set_runtime_owner(home_stat.st_uid, home_stat.st_gid);
    utils::set_runtime_path_override(home, utils::get_config_dir());
  }
  utils::fix_config_dir_ownership();
}

std::string helper_unavailable_connect_message() {
  return "Helper daemon is not available. Install the helper service before starting the desktop client.";
}

std::string helper_unavailable_disconnect_message() {
  return "Helper daemon is not available. Install the helper service before disconnecting managed sessions.";
}

nlohmann::json preflight_connect_platform_checks(const Config & /*cfg*/) {
  // No platform-specific driver checks on Linux.
  return nlohmann::json{};
}

nlohmann::json try_connect_direct_fallback(const Config &cfg,
                                            const std::string &password) {
  prepare_direct_fallback_runtime();
  int result = vpn::start_with_password(cfg, password, 0);
  if (result != 0) {
    if (result == vpn::kVpnInitialConnectFailedExitCode) {
      return nlohmann::json{{"ok", false},
                            {"code", "auth_failed"},
                            {"error", "VPN authentication failed or the server rejected the connection."}};
    }
    return nlohmann::json{{"ok", false}, {"error", "Failed to start VPN"}};
  }
  vpn::RuntimeStatusSnapshot snapshot = vpn::read_runtime_status_snapshot();
  return nlohmann::json{{"ok", true},
                        {"_direct_fallback", true},
                        {"_snapshot_data",
                         nlohmann::json{{"running", snapshot.running},
                                        {"pid", snapshot.pid},
                                        {"supervisor_pid", snapshot.supervisor_pid},
                                        {"network_ready", snapshot.network_ready},
                                        {"interface", snapshot.interface_name},
                                        {"internal_ip", snapshot.internal_ip}}}};
}

nlohmann::json try_disconnect_direct_fallback(bool allow_direct_fallback) {
  if (!allow_direct_fallback)
    return nlohmann::json{};

  vpn::RuntimeStatusSnapshot snapshot = vpn::read_runtime_status_snapshot();
  if (!snapshot.running)
    return nlohmann::json{{"ok", true}, {"_not_running", true}};

  if (!vpn::stop_direct_session()) {
    return nlohmann::json{{"ok", false}, {"error", "Failed to stop VPN"}};
  }
  return nlohmann::json{{"ok", true}, {"_direct_fallback", true}};
}

nlohmann::json status_fallback_without_helper(const Config & /*cfg*/) {
  vpn::RuntimeStatusSnapshot snapshot = vpn::read_runtime_status_snapshot();
  nlohmann::json result;
  result["_snapshot"] = true;
  result["_running"] = snapshot.running;
  result["_snapshot_data"] = nlohmann::json{
      {"running", snapshot.running},
      {"pid", snapshot.pid},
      {"supervisor_pid", snapshot.supervisor_pid},
      {"network_ready", snapshot.network_ready},
      {"interface", snapshot.interface_name},
      {"internal_ip", snapshot.internal_ip},
  };
  return result;
}

} // namespace platform
} // namespace ecnuvpn
