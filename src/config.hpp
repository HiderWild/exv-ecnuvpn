#pragma once

#include "platform/common/config_defaults.hpp"

#include <nlohmann/json.hpp>
#include <string>
#include <vector>

namespace ecnuvpn {

struct Config {
  std::string server = "https://vpn-ct.ecnu.edu.cn";
  std::string username = "";
  std::string password =
      ""; // AES-256-CBC ciphertext (base64); empty if remember_password=false
  int mtu = 1290;
  std::string useragent = platform::config_defaults().useragent;
  bool disable_dtls = platform::config_defaults().disable_dtls;
  bool remember_password = true; // false = prompt hidden input at connect time
  std::vector<std::string> routes = {
      "49.52.4.0/25",      "59.78.176.0/20",  "59.78.199.0/21",
      "58.198.176.128/25", "219.228.60.69",   "59.78.189.128/25",
      "219.228.63.0/21",   "202.120.80.0/20", "222.66.117.0/24"};
  std::vector<std::string> extra_args;
  std::string log_file = platform::config_defaults().log_file;
  int webui_port = 18080;
  std::string webui_bind = "127.0.0.1";
  bool webui_enabled = platform::config_defaults().webui_enabled;
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
