#include "config.hpp"
#include "logger.hpp"
#include "platform/common/tunnel_script.hpp"
#include "platform/common/config_defaults.hpp"
#include "tunnel.hpp"
#include "utils.hpp"

#include <cstdio>
#include <filesystem>
#include <fstream>
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

ecnuvpn::Config g_loaded_config;
std::vector<std::string> g_commands;

} // namespace

namespace ecnuvpn {
namespace platform {

const ConfigDefaults &config_defaults() {
       static const ConfigDefaults defaults{};
       return defaults;
}

} // namespace platform

namespace config {

Config load() { return g_loaded_config; }

} // namespace config

namespace logger {

void init() {}
void info(const std::string &) {}
void error(const std::string &) {}
void warn(const std::string &) {}
void show_logs(int) {}

} // namespace logger

namespace utils {

void print_success(const std::string &) {}
void print_error(const std::string &) {}
void print_info(const std::string &) {}
void print_warning(const std::string &) {}
void print_header(const std::string &) {}

std::string expand_home(const std::string &path) { return path; }
std::string get_redirect_path() { return ""; }
std::string get_config_dir() { return ""; }
bool set_config_dir(const std::string &) { return true; }
std::string get_config_path() { return ""; }
std::string get_pid_path() { return ""; }
std::string get_log_path() { return ""; }
std::string get_tunnel_path() { return ""; }
std::string get_supervisor_pid_path() { return ""; }
std::string get_route_ready_path() { return "route-ready"; }
std::string get_effective_home() { return ""; }
std::string get_home_for_uid(uid_t) { return ""; }
std::string get_username_for_uid(uid_t) { return ""; }
std::string get_config_dir_for_uid(uid_t) { return ""; }
void set_runtime_path_override(const std::string &, const std::string &) {}
void clear_runtime_path_override() {}
void set_runtime_owner(uid_t, gid_t) {}
void clear_runtime_owner() {}
bool has_runtime_owner() { return false; }
uid_t get_runtime_owner_uid() { return 0; }
gid_t get_runtime_owner_gid() { return 0; }
bool sync_owner(const std::string &) { return true; }
std::string get_executable_path() { return ""; }

bool fix_config_dir_ownership() { return true; }

bool file_exists(const std::string &) { return false; }
bool ensure_dir(const std::string &) { return true; }
std::string read_file(const std::string &) { return ""; }
bool write_file(const std::string &, const std::string &) { return true; }

std::string get_bundled_runtime_dir() { return ""; }
std::string get_bundled_openconnect_path() { return ""; }
std::string get_bundled_wintun_path() { return ""; }
std::string get_bundled_tap_installer_path() { return ""; }
std::string get_openconnect_path(const std::string &) { return ""; }
bool check_openconnect(const std::string &) { return true; }
bool check_root() { return false; }
bool get_interface_traffic(const std::string &, uint64_t *, uint64_t *) {
  return false;
}
int run_command(const std::string &cmd) {
  g_commands.push_back(cmd);
  return 0;
}
std::string run_command_output(const std::string &) { return ""; }
std::string shell_quote(const std::string &value) { return "'" + value + "'"; }
std::vector<std::string> split_lines(const std::string &) { return {}; }

std::string trim(const std::string &s) { return s; }

} // namespace utils
} // namespace ecnuvpn

int main() {
  bool ok = true;

  ecnuvpn::Config cfg;
  cfg.server = "https://59.78.176.10";
  cfg.routes = {"59.78.176.0/20", "10.0.0.0/8"};

  std::string script = ecnuvpn::tunnel::generate(cfg);

#ifdef _WIN32
  ok = expect(script.find("serverRouteExceptions") != std::string::npos,
              "generated Windows script should declare server route exceptions") &&
       ok;
  ok = expect(script.find("59.78.176.10") != std::string::npos,
              "generated Windows script should include the matching server exception IP") &&
       ok;
  ok = expect(script.find("route.exe delete") != std::string::npos,
              "generated Windows script should own route cleanup command text") &&
       ok;
#elif defined(__APPLE__)
  ok = expect(script.find("SERVER_EXCEPTIONS=\"59.78.176.10\"") !=
                  std::string::npos,
              "generated macOS script should include the matching server exception IP") &&
       ok;
  ok = expect(script.find("route -n delete \"$route\"") != std::string::npos,
              "generated macOS script should own route cleanup command text") &&
       ok;

  g_loaded_config = cfg;
  g_commands.clear();
  ecnuvpn::tunnel::cleanup_routes();

  ok = expect(g_commands.size() == 3,
              "cleanup should remove both custom routes and the server exception") &&
       ok;
  ok = expect(g_commands[0] ==
                  "route -n delete '59.78.176.0/20' >/dev/null 2>&1",
              "cleanup should delegate custom route deletion to the macOS platform adapter") &&
       ok;
  ok = expect(g_commands[1] ==
                  "route -n delete '10.0.0.0/8' >/dev/null 2>&1",
              "cleanup should preserve custom route ordering on macOS") &&
       ok;
  ok = expect(g_commands[2] ==
                  "route -n delete '59.78.176.10' >/dev/null 2>&1",
              "cleanup should delegate server exception deletion to the macOS platform adapter") &&
       ok;
#else
  ok = expect(script.find("SERVER_EXCEPTIONS=\"59.78.176.10\"") !=
                  std::string::npos,
              "generated Linux script should include the matching server exception IP") &&
       ok;
  ok = expect(script.find("ip route del \"$route\"") != std::string::npos,
              "generated Linux script should own route cleanup command text") &&
       ok;

  g_loaded_config = cfg;
  g_commands.clear();
  ecnuvpn::tunnel::cleanup_routes();

  ok = expect(g_commands.size() == 3,
              "cleanup should remove both custom routes and the server exception") &&
       ok;
  ok = expect(g_commands[0] ==
                  "ip route del '59.78.176.0/20' >/dev/null 2>&1",
              "cleanup should delegate custom route deletion to the Linux platform adapter") &&
       ok;
  ok = expect(g_commands[1] ==
                  "ip route del '10.0.0.0/8' >/dev/null 2>&1",
              "cleanup should preserve custom route ordering on Linux") &&
       ok;
  ok = expect(g_commands[2] ==
                  "ip route del '59.78.176.10' >/dev/null 2>&1",
              "cleanup should delegate server exception deletion to the Linux platform adapter") &&
       ok;
#endif

  ecnuvpn::Config no_exception_cfg = cfg;
  no_exception_cfg.server = "https://203.0.113.10";
  std::string no_exception_script = ecnuvpn::tunnel::generate(no_exception_cfg);

  ok = expect(no_exception_script.find("203.0.113.10") == std::string::npos,
              "generated script should not invent unmatched server route exceptions") &&
       ok;

  std::filesystem::path temp_dir = std::filesystem::temp_directory_path();
  std::filesystem::path log_path =
      temp_dir / "exv-native-log-scraping-contract.log";
  std::filesystem::path ready_path =
      temp_dir / "exv-native-log-scraping-contract.ready";
  std::remove(log_path.string().c_str());
  std::remove(ready_path.string().c_str());

  {
    std::ofstream log(log_path);
    log << "Starting VPN: vpn.example.edu user=test\n"
        << "Using Wintun device 'ECNUVPN-NATIVE', index 77\n"
        << "Configured as 10.77.0.8, with SSL connected\n";
  }

  ecnuvpn::platform::TunnelScriptContext native_context;
  native_context.vpn_engine = "native";
  native_context.route_ready_path = ready_path.string();
  native_context.custom_routes = cfg.routes;
  native_context.configured_mtu = cfg.mtu;

  auto native_result = ecnuvpn::platform::configure_from_openconnect_log(
      native_context, log_path.string());
  ok = expect(!native_result.ok,
              "native mode should reject OpenConnect log scraping") &&
       ok;
  ok = expect(native_result.code == "native_log_scraping_disabled",
              "native mode should return the deterministic disabled code") &&
       ok;
  ok = expect(!std::filesystem::exists(ready_path),
              "native mode should not parse tunnel metadata into a ready file") &&
       ok;

#ifdef _WIN32
  std::remove(ready_path.string().c_str());
  ecnuvpn::platform::TunnelScriptContext legacy_context = native_context;
  legacy_context.vpn_engine = "legacy_openconnect";
  auto legacy_result = ecnuvpn::platform::configure_from_openconnect_log(
      legacy_context, log_path.string());
  ok = expect(legacy_result.ok,
              "legacy OpenConnect mode should preserve log fallback parsing") &&
       ok;
  ok = expect(std::filesystem::exists(ready_path),
              "legacy OpenConnect mode should still write route-ready metadata") &&
       ok;
#endif

  std::remove(log_path.string().c_str());
  std::remove(ready_path.string().c_str());

  return ok ? 0 : 1;
}
