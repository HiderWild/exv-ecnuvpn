#pragma once

#include <string>

namespace ecnuvpn {
namespace platform {

std::string join_path(const std::string &base, const std::string &component);
std::string redirect_path_for_home(const std::string &home);
std::string default_config_dir_for_home(const std::string &home);
std::string home_for_uid(unsigned int uid);
std::string username_for_uid(unsigned int uid);
std::string effective_home();
std::string config_path(const std::string &config_dir);
std::string pid_path(const std::string &config_dir);
std::string log_path(const std::string &config_dir);
std::string tunnel_path(const std::string &config_dir);
std::string supervisor_pid_path(const std::string &config_dir);
std::string route_ready_path(const std::string &config_dir);
bool sync_owner(const std::string &path, unsigned int uid, unsigned int gid);
bool fix_config_dir_ownership(const std::string &dir,
                              const std::string &effective_home);

} // namespace platform
} // namespace ecnuvpn