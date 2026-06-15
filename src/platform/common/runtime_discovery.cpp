#include "platform/common/runtime_discovery.hpp"

#include "platform/common/file_system.hpp"
#include "platform/common/path_utils.hpp"
#include "platform/common/process_utils.hpp"
#include "utils/strings.hpp"

#include <cstdlib>
#include <filesystem>
#ifdef _WIN32
#include <io.h>
#else
#include <unistd.h>
#endif
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
    dirs.push_back(join_path(exec_dir.string(), "openconnect"));
    dirs.push_back(join_path(join_path(exec_dir.string(), "runtime"),
                             "openconnect"));
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

std::string get_bundled_openconnect_path() {
  std::vector<std::string> candidates;
  for (const auto &dir : candidate_runtime_dirs()) {
    if (dir.empty()) {
      continue;
    }
#ifdef _WIN32
    candidates.push_back(join_path(dir, "openconnect.exe"));
#else
    candidates.push_back(join_path(dir, "openconnect"));
#endif
  }
  return first_existing_file(candidates);
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

std::string get_openconnect_path(const std::string &runtime_mode) {
  const char *env_openconnect = std::getenv("ECNUVPN_OPENCONNECT");
  if (env_openconnect && *env_openconnect && file_exists(env_openconnect)) {
    return env_openconnect;
  }

  if (runtime_mode != "system") {
    std::string bundled = get_bundled_openconnect_path();
    if (!bundled.empty()) {
#ifdef __APPLE__
      std::string verify_cmd =
          "codesign --verify --strict " + shell_quote(bundled) +
          " >/dev/null 2>&1";
      if (std::system(verify_cmd.c_str()) != 0) {
        bundled.clear();
      }
#endif
    }
    if (!bundled.empty()) {
      return bundled;
    }
  }

#ifdef __APPLE__
  const char *candidates[] = {"/opt/homebrew/bin/openconnect",
                              "/usr/local/bin/openconnect",
                              "/usr/bin/openconnect",
                              "/bin/openconnect"};
#elif defined(_WIN32)
  const char *candidates[] = {
      "C:\\Program Files\\OpenConnect\\openconnect.exe",
      "C:\\Program Files (x86)\\OpenConnect\\openconnect.exe",
      "openconnect.exe"};
#else
  const char *candidates[] = {"/usr/sbin/openconnect",
                              "/usr/bin/openconnect",
                              "/sbin/openconnect",
                              "/usr/local/bin/openconnect"};
#endif
  for (const char *candidate : candidates) {
#ifdef _WIN32
    if (candidate && _access(candidate, 0) == 0) {
#else
    if (candidate && access(candidate, X_OK) == 0) {
#endif
      return candidate;
    }
  }

#ifdef _WIN32
  std::string resolved =
      exv::utils::trim(run_command_output("where openconnect.exe 2>nul"));
  std::string::size_type newline = resolved.find_first_of("\r\n");
  if (newline != std::string::npos) {
    resolved.resize(newline);
  }
  if (!resolved.empty() && _access(resolved.c_str(), 0) == 0) {
    return resolved;
  }
#else
  std::string resolved =
      exv::utils::trim(run_command_output("command -v openconnect 2>/dev/null"));
  if (!resolved.empty() && access(resolved.c_str(), X_OK) == 0) {
    return resolved;
  }
#endif
  return "";
}

bool check_openconnect(const std::string &runtime_mode) {
  return !get_openconnect_path(runtime_mode).empty();
}

} // namespace ecnuvpn::platform

