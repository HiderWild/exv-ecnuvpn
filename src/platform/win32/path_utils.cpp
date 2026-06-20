#include "platform/common/path_utils.hpp"

#include <cstdlib>

namespace exv {
namespace platform {
namespace {

std::string get_windows_local_app_data_home(const std::string &home) {
  const char *local_app_data = std::getenv("LOCALAPPDATA");
  if (local_app_data && *local_app_data)
    return local_app_data;
  if (!home.empty())
    return join_path(join_path(home, "AppData"), "Local");
  return "";
}

std::string profile_root_for_home(const std::string &home) {
  std::string base = get_windows_local_app_data_home(home);
  if (base.empty())
    return "";
  return join_path(base, "EXV");
}

} // namespace

std::string join_path(const std::string &base, const std::string &component) {
  if (base.empty())
    return component;
  if (base.back() == '/' || base.back() == '\\')
    return base + component;
  return base + "\\" + component;
}

std::string redirect_path_for_home(const std::string &home) {
  std::string base = profile_root_for_home(home);
  if (base.empty())
    return "";
  return join_path(base, "profile.redirect");
}

std::string default_config_dir_for_home(const std::string &home) {
  std::string base = profile_root_for_home(home);
  if (base.empty())
    return "";
  return join_path(join_path(base, "profile"), "default");
}

std::string home_for_uid(unsigned int uid) {
  (void)uid;
  const char *home = std::getenv("USERPROFILE");
  if (home && *home)
    return home;
  const char *home_drive = std::getenv("HOMEDRIVE");
  const char *home_path = std::getenv("HOMEPATH");
  if (home_drive && home_path)
    return std::string(home_drive) + home_path;
  return "";
}

std::string username_for_uid(unsigned int uid) {
  (void)uid;
  const char *username = std::getenv("USERNAME");
  return username ? username : "";
}

std::string effective_home() {
  return home_for_uid(0);
}

std::string config_path(const std::string &config_dir) {
  return join_path(config_dir, "config.json");
}

std::string pid_path(const std::string &config_dir) {
  return join_path(config_dir, "exv.pid");
}

std::string log_path(const std::string &config_dir) {
  return join_path(config_dir, "exv.log");
}

std::string tunnel_path(const std::string &config_dir) {
  return join_path(config_dir, "tunnel.js");
}

std::string route_ready_path(const std::string &config_dir) {
  return join_path(config_dir, "route-ready");
}

bool sync_owner(const std::string &path, unsigned int uid, unsigned int gid) {
  (void)path;
  (void)uid;
  (void)gid;
  return true;
}

bool fix_config_dir_ownership(const std::string &dir,
                              const std::string &effective_home_path) {
  (void)dir;
  (void)effective_home_path;
  return true;
}

} // namespace platform
} // namespace exv
