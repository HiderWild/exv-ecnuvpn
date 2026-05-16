#pragma once

#include <nlohmann/json.hpp>
#include <cstdlib>
#include <string>
#include <vector>

namespace ecnuvpn {

#ifdef __APPLE__
static constexpr bool DEFAULT_DISABLE_DTLS = true;
#else
static constexpr bool DEFAULT_DISABLE_DTLS = false;
#endif

#ifdef _WIN32
static inline std::string default_log_file_path() {
  const char *appdata = std::getenv("APPDATA");
  if (appdata && *appdata)
    return std::string(appdata) + "\\ecnuvpn\\ecnuvpn.log";
  const char *home = std::getenv("USERPROFILE");
  if (home && *home)
    return std::string(home) + "\\AppData\\Roaming\\ecnuvpn\\ecnuvpn.log";
  return "C:\\ProgramData\\ecnuvpn\\ecnuvpn.log";
}
#else
static inline std::string default_log_file_path() {
  return "~/.ecnuvpn/ecnuvpn.log";
}
#endif

struct Config {
  std::string server = "https://vpn-ct.ecnu.edu.cn";
  std::string username = "";
  std::string password =
      ""; // AES-256-CBC ciphertext (base64); empty if remember_password=false
  int mtu = 1290;
  #ifdef __APPLE__
  std::string useragent = "AnyConnect Darwin_x86_64 4.10.05095";
#elif defined(_WIN32)
  std::string useragent = "AnyConnect Win_x86_64 4.10.05095";
#else
  std::string useragent = "AnyConnect Linux_x86_64 4.10.05095";
#endif
  bool disable_dtls = DEFAULT_DISABLE_DTLS;
  bool remember_password = true; // false = prompt hidden input at connect time
  std::vector<std::string> routes = {
      "49.52.4.0/25",      "59.78.176.0/20",  "59.78.199.0/21",
      "58.198.176.128/25", "219.228.60.69",   "59.78.189.128/25",
      "219.228.63.0/21",   "202.120.80.0/20", "222.66.117.0/24"};
  std::vector<std::string> extra_args;
  std::string log_file = default_log_file_path();
  int webui_port = 18080;
  std::string webui_bind = "127.0.0.1";
  bool webui_enabled = true;
  std::string openconnect_runtime = "bundled";
  std::string windows_tunnel_driver = "auto";
  std::string windows_tap_interface = "";

  NLOHMANN_DEFINE_TYPE_INTRUSIVE_WITH_DEFAULT(Config, server, username,
                                              password, mtu, useragent,
                                              disable_dtls, remember_password, routes,
                                              extra_args, log_file,
                                              webui_port, webui_bind, webui_enabled,
                                              openconnect_runtime,
                                              windows_tunnel_driver,
                                              windows_tap_interface)
};

namespace config {

// Load config (creates default + key on first run)
Config load();
bool save(const Config &cfg);

// Display config with password/key status
void show(const Config &cfg);

// Import from external JSON file (merges, re-encrypts password if present)
Config import_from(const std::string &path);

// Set a key-value. For "password", prompts hidden input and encrypts.
bool set_value(Config &cfg, const std::string &key,
               const std::string &value = "");

// Decrypt the stored password ciphertext using current key file.
// Returns "" and prints an error if key is missing/corrupt.
std::string get_plaintext_password(const Config &cfg);

// Reset config to defaults (keeps key file intact)
Config reset();

// Route management
bool add_route(Config &cfg, const std::string &route);
bool remove_route(Config &cfg, const std::string &route);
void list_routes(const Config &cfg);

// Key management subcommands
void key_show();
bool key_reset();

} // namespace config
} // namespace ecnuvpn
