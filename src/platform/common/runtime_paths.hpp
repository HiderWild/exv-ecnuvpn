#pragma once

#include <string>

namespace ecnuvpn::platform {

std::string expand_home(const std::string &path);
std::string get_redirect_path();
std::string get_config_dir();
bool set_config_dir(const std::string &dir);
std::string get_config_path();
std::string get_log_path();
std::string get_tunnel_path();
std::string get_pid_path();
std::string get_supervisor_pid_path();
std::string get_route_ready_path();
std::string get_effective_home();
std::string get_home_for_uid(unsigned int uid);
std::string get_username_for_uid(unsigned int uid);
std::string get_config_dir_for_uid(unsigned int uid);
void set_runtime_path_override(const std::string &home,
                               const std::string &config_dir);
void clear_runtime_path_override();
void set_runtime_owner(unsigned int uid, unsigned int gid);
void clear_runtime_owner();
bool has_runtime_owner();
unsigned int get_runtime_owner_uid();
unsigned int get_runtime_owner_gid();
bool sync_owner(const std::string &path);
bool fix_runtime_config_dir_ownership();

} // namespace ecnuvpn::platform
