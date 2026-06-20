#include "platform/common/path_utils.hpp"

#include <cstdlib>
#include <filesystem>
#include <pwd.h>
#include <sys/stat.h>
#include <unistd.h>

namespace exv {
namespace platform {
namespace {

std::string expand_home_with_base(const std::string &path,
                                  const std::string &home) {
  if (!path.empty() && path[0] == '~' && !home.empty())
    return home + path.substr(1);
  return path;
}

} // namespace

std::string join_path(const std::string &base, const std::string &component) {
  if (base.empty())
    return component;
  if (base.back() == '/')
    return base + component;
  return base + "/" + component;
}

std::string redirect_path_for_home(const std::string &home) {
  return expand_home_with_base("~/.exv_home", home);
}

std::string default_config_dir_for_home(const std::string &home) {
  return expand_home_with_base("~/.exv", home);
}

std::string home_for_uid(unsigned int uid) {
  struct passwd *pw = getpwuid(static_cast<uid_t>(uid));
  if (pw && pw->pw_dir)
    return pw->pw_dir;
  return "";
}

std::string username_for_uid(unsigned int uid) {
  struct passwd *pw = getpwuid(static_cast<uid_t>(uid));
  if (pw && pw->pw_name)
    return pw->pw_name;
  return "";
}

std::string effective_home() {
  const char *sudo_user = std::getenv("SUDO_USER");
  if (sudo_user && *sudo_user) {
    struct passwd *pw = getpwnam(sudo_user);
    if (pw && pw->pw_dir)
      return pw->pw_dir;
  }

  const char *home = std::getenv("HOME");
  if (home && *home)
    return home;

  struct passwd *pw = getpwuid(getuid());
  if (pw && pw->pw_dir)
    return pw->pw_dir;

  return "";
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
  return join_path(config_dir, "tunnel.sh");
}

std::string route_ready_path(const std::string &config_dir) {
  return join_path(config_dir, "route-ready");
}

bool sync_owner(const std::string &path, unsigned int uid, unsigned int gid) {
  return chown(path.c_str(), static_cast<uid_t>(uid),
               static_cast<gid_t>(gid)) == 0;
}

bool fix_config_dir_ownership(const std::string &dir,
                              const std::string &effective_home_path) {
  if (dir.empty() || ::access(dir.c_str(), F_OK) != 0)
    return true;

  struct stat st {};
  if (stat(dir.c_str(), &st) != 0)
    return false;

  uid_t expected_uid = getuid();
  if (expected_uid == 0 && !effective_home_path.empty()) {
    struct stat home_st {};
    if (stat(effective_home_path.c_str(), &home_st) == 0)
      expected_uid = home_st.st_uid;
  }

  if (st.st_uid == expected_uid)
    return true;

  if (chown(dir.c_str(), expected_uid, static_cast<gid_t>(-1)) != 0)
    return false;

  for (const auto &entry : std::filesystem::directory_iterator(dir)) {
    chown(entry.path().c_str(), expected_uid, static_cast<gid_t>(-1));
  }
  return true;
}

} // namespace platform
} // namespace exv
