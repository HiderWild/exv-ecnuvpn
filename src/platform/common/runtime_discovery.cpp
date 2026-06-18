#include "platform/common/runtime_discovery.hpp"

#include "platform/common/file_system.hpp"
#include "platform/common/path_utils.hpp"
#include "platform/common/process_utils.hpp"

#include <cstdlib>
#include <filesystem>
#include <vector>

namespace ecnuvpn::platform {
namespace {

std::vector<std::string> candidate_runtime_dirs() {
  std::vector<std::string> dirs;

  const char *env_runtime_dir = std::getenv("ECNUVPN_RUNTIME_DIR");
  if (env_runtime_dir && *env_runtime_dir) {
    dirs.push_back(env_runtime_dir);
  }

  std::string exec_path = get_executable_path();
  if (!exec_path.empty()) {
    std::filesystem::path exec_dir =
        std::filesystem::path(exec_path).parent_path();
    dirs.push_back(exec_dir.string());
    dirs.push_back(join_path(exec_dir.string(), "runtime"));
  }

  return dirs;
}

std::string first_existing_file(const std::vector<std::string> &paths) {
  for (const auto &path : paths) {
    if (!path.empty() && file_exists(path)) {
      return path;
    }
  }
  return "";
}

} // namespace

std::string get_bundled_runtime_dir() {
  for (const auto &dir : candidate_runtime_dirs()) {
    if (!dir.empty() && file_exists(dir)) {
      return dir;
    }
  }
  return "";
}

std::string get_bundled_wintun_path() {
#ifdef _WIN32
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (!dir.empty()) {
      candidates.push_back(join_path(dir, "wintun.dll"));
    }
  }
  return first_existing_file(candidates);
#else
  return "";
#endif
}

std::string get_bundled_tap_installer_path() {
#ifdef _WIN32
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (dir.empty()) {
      continue;
    }
    candidates.push_back(join_path(dir, "tap-windows-installer.exe"));
    candidates.push_back(join_path(dir, "tap-windows-amd64.exe"));
    candidates.push_back(join_path(dir, "tap-windows-x86.exe"));
    candidates.push_back(join_path(dir, "tap-windows/OemVista.inf"));
    candidates.push_back(join_path(dir, "tap/OemVista.inf"));
  }
  return first_existing_file(candidates);
#else
  return "";
#endif
}

} // namespace ecnuvpn::platform

