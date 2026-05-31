#pragma once

#include "config_manager.hpp"

#include <string>

namespace ecnuvpn {
namespace config_api {

// Non-interactive config field setter.
// Returns empty string on success, error message on failure.
std::string config_set(config::ConfigManager& mgr, const std::string& key,
                       const std::string& value);

// Encrypt plaintext password and store it.
// Enables remember_password if currently disabled.
// Returns empty string on success, error message on failure.
std::string config_set_password(config::ConfigManager& mgr,
                                const std::string& plaintext);

// Clear stored password, disable remember_password, and delete the key file.
// Returns empty string on success, error message on failure.
std::string config_clear_password_and_key(config::ConfigManager& mgr);

// Reset config to defaults (preserves key file)
void config_reset(config::ConfigManager& mgr);

// Import config from a JSON string. Merges into existing config.
// Returns empty string on success, error message on failure.
std::string config_import(config::ConfigManager& mgr, const std::string& json_str);

// Route management
std::string route_add(config::ConfigManager& mgr, const std::string& cidr);
std::string route_remove(config::ConfigManager& mgr, const std::string& cidr);
void route_reset_defaults(config::ConfigManager& mgr);

// Key management
std::string key_status();
void key_reset_noninteractive();

} // namespace config_api
} // namespace ecnuvpn
