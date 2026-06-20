#include "platform/common/runtime_paths.hpp"

#include "platform/common/file_system.hpp"
#include "platform/common/path_utils.hpp"
#include "utils/strings.hpp"

#include <fstream>

namespace exv::platform {
namespace {

std::string runtime_home_override;
std::string runtime_config_dir_override;
unsigned int runtime_owner_uid = static_cast<unsigned int>(-1);
unsigned int runtime_owner_gid = static_cast<unsigned int>(-1);

std::string expand_home_with_base(const std::string &path,
                                  const std::string &home) {
  if (!path.empty() && path[0] == '~' && !home.empty()) {
    return home + path.substr(1);
  }
  return path;
}

std::string get_config_dir_for_home(const std::string &home) {
  std::string default_dir = default_config_dir_for_home(home);
  if (default_dir.empty()) {
    return "";
  }

  std::string redirect = redirect_path_for_home(home);
  if (!redirect.empty()) {
    std::ifstream rf(redirect);
    if (rf.is_open()) {
      std::string dir;
      std::getline(rf, dir);
      dir = exv::utils::trim(dir);
      if (!dir.empty()) {
        return expand_home_with_base(dir, home);
      }
    }
  }

  return default_dir;
}

} // namespace

std::string get_home_for_uid(unsigned int uid) { return home_for_uid(uid); }

std::string get_username_for_uid(unsigned int uid) {
  return username_for_uid(uid);
}

std::string get_effective_home() {
  if (!runtime_home_override.empty()) {
    return runtime_home_override;
  }
  return effective_home();
}

std::string expand_home(const std::string &path) {
  return expand_home_with_base(path, get_effective_home());
}

std::string get_redirect_path() {
  return redirect_path_for_home(get_effective_home());
}

std::string get_config_dir() {
  if (!runtime_config_dir_override.empty()) {
    return runtime_config_dir_override;
  }
  return get_config_dir_for_home(get_effective_home());
}

std::string get_config_dir_for_uid(unsigned int uid) {
  return get_config_dir_for_home(get_home_for_uid(uid));
}

void set_runtime_path_override(const std::string &home,
                               const std::string &config_dir) {
  runtime_home_override = home;
  runtime_config_dir_override = config_dir.empty()
                                    ? get_config_dir_for_home(home)
                                    : expand_home_with_base(config_dir, home);
}

void clear_runtime_path_override() {
  runtime_home_override.clear();
  runtime_config_dir_override.clear();
}

void set_runtime_owner(unsigned int uid, unsigned int gid) {
  runtime_owner_uid = uid;
  runtime_owner_gid = gid;
}

void clear_runtime_owner() {
  runtime_owner_uid = static_cast<unsigned int>(-1);
  runtime_owner_gid = static_cast<unsigned int>(-1);
}

bool has_runtime_owner() {
  return runtime_owner_uid != static_cast<unsigned int>(-1) &&
         runtime_owner_gid != static_cast<unsigned int>(-1);
}

unsigned int get_runtime_owner_uid() { return runtime_owner_uid; }

unsigned int get_runtime_owner_gid() { return runtime_owner_gid; }

bool sync_owner(const std::string &path) {
  if (!has_runtime_owner()) {
    return true;
  }
  if (!file_exists(path)) {
    return false;
  }
  return sync_owner(path, runtime_owner_uid, runtime_owner_gid);
}

bool set_config_dir(const std::string &dir) {
  std::string expanded = expand_home(dir);
  if (!ensure_dir(expanded)) {
    return false;
  }
  std::ofstream wf(get_redirect_path());
  if (!wf.is_open()) {
    return false;
  }
  wf << dir;
  return wf.good() && sync_owner(get_redirect_path());
}

std::string get_config_path() { return config_path(get_config_dir()); }

std::string get_log_path() { return log_path(get_config_dir()); }

std::string get_tunnel_path() { return tunnel_path(get_config_dir()); }

std::string get_pid_path() { return pid_path(get_config_dir()); }

std::string get_route_ready_path() { return route_ready_path(get_config_dir()); }

bool fix_runtime_config_dir_ownership() {
  return fix_config_dir_ownership(get_config_dir(), get_effective_home());
}

} // namespace exv::platform
