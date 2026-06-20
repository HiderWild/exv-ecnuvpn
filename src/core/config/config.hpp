#pragma once

#include "generated/distribution_config.hpp"
#include "platform/common/config_defaults.hpp"

#include <nlohmann/json.hpp>
#include <string>
#include <vector>

namespace exv {
namespace config_detail {

inline std::vector<std::string> default_distribution_routes() {
  std::vector<std::string> routes;
  routes.reserve(distribution::kDefaultRoutes.size());
  for (const auto route : distribution::kDefaultRoutes) {
    routes.emplace_back(route);
  }
  return routes;
}

} // namespace config_detail

struct Config {
  std::string server = std::string(distribution::kDefaultVpnServer);
  std::string username = "";
  std::string password =
      ""; // AES-256-CBC ciphertext (base64); empty if remember_password=false
  int mtu = 1290;
  std::string useragent = platform::config_defaults().useragent;
  bool disable_dtls = platform::config_defaults().disable_dtls;
  bool remember_password = false; // false = prompt hidden input at connect time
  std::vector<std::string> routes = config_detail::default_distribution_routes();
  std::vector<std::string> extra_args;
  std::string log_file = platform::config_defaults().log_file;
  std::string vpn_engine = "native";
  std::string windows_tunnel_driver = "auto";
  std::string windows_tap_interface = "";
  bool auto_reconnect = true;
  bool minimal_mode = false;
  bool service_install_prompt_seen = false;
  bool minimal_install_service_before_connect = true;
  bool include_class_a_private_routes = false;
  bool include_class_b_private_routes = false;
  bool launch_at_login = false;
  bool auto_connect_on_launch = false;

  NLOHMANN_DEFINE_TYPE_INTRUSIVE_WITH_DEFAULT(Config, server, username,
                                              password, mtu, useragent,
                                              disable_dtls, remember_password, routes,
                                              extra_args, log_file,
                                              vpn_engine,
                                              windows_tunnel_driver,
                                              windows_tap_interface,
                                              auto_reconnect,
                                              minimal_mode,
                                              service_install_prompt_seen,
                                              minimal_install_service_before_connect,
                                              include_class_a_private_routes,
                                              include_class_b_private_routes,
                                              launch_at_login,
                                              auto_connect_on_launch)
};

inline void normalize_native_only(Config &cfg) { cfg.vpn_engine = "native"; }

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
} // namespace exv
