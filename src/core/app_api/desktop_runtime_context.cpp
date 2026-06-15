#include "core/app_api/desktop_runtime_context.hpp"

#include "core/app_api/desktop_json.hpp"
#include "platform/common/runtime_paths.hpp"

#ifndef _WIN32
#include <sys/stat.h>
#endif

namespace ecnuvpn {
namespace app_api {

void apply_desktop_runtime_context(const nlohmann::json &payload) {
  if (!payload.is_object()) {
    return;
  }

  const std::string home = json_string(payload, "home");
  const std::string config_dir = json_string(payload, "config_dir");
  if (home.empty() && config_dir.empty()) {
    return;
  }

  platform::set_runtime_path_override(
      home.empty() ? platform::get_effective_home() : home, config_dir);

#ifndef _WIN32
  const std::string owner_home =
      home.empty() ? platform::get_effective_home() : home;
  struct stat home_stat {};
  if (!owner_home.empty() && stat(owner_home.c_str(), &home_stat) == 0) {
    platform::set_runtime_owner(home_stat.st_uid, home_stat.st_gid);
  }
#endif
}

void add_desktop_owner_context(nlohmann::json &request) {
#ifndef _WIN32
  if (!platform::has_runtime_owner()) {
    return;
  }
  request["owner_uid"] =
      static_cast<unsigned int>(platform::get_runtime_owner_uid());
  request["owner_gid"] =
      static_cast<unsigned int>(platform::get_runtime_owner_gid());
#else
  (void)request;
#endif
}

} // namespace app_api
} // namespace ecnuvpn
