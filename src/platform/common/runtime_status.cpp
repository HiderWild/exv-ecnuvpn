#include "platform/common/runtime_status.hpp"

#include "platform/common/status_models.hpp"
#include "utils.hpp"

#ifndef ECNUVPN_VERSION
#define ECNUVPN_VERSION "dev"
#endif

namespace ecnuvpn {
namespace platform {
namespace {

std::string first_nonempty_line(const std::string &text) {
  for (const auto &line : utils::split_lines(text)) {
    if (!line.empty())
      return line;
  }
  return "";
}

std::string openconnect_version(const std::string &path) {
  if (path.empty())
    return "";
#ifdef _WIN32
  return first_nonempty_line(
      utils::run_command_output(utils::shell_quote(path) + " --version 2>nul"));
#else
  return first_nonempty_line(
      utils::run_command_output(utils::shell_quote(path) + " --version 2>/dev/null"));
#endif
}

} // namespace

nlohmann::json runtime_status_json(const Config &cfg) {
  std::string bundled_path = utils::get_bundled_openconnect_path();
  std::string system_path = utils::get_openconnect_path("system");
  std::string resolved_path = utils::get_openconnect_path(cfg.openconnect_runtime);

  nlohmann::json legacy{{"engine", "legacy_openconnect"},
                        {"mode", cfg.openconnect_runtime},
                        {"available", !resolved_path.empty()},
                        {"source", runtime_source_from_paths(resolved_path,
                                                             bundled_path,
                                                             system_path)},
                        {"path", resolved_path},
                        {"bundled_path", bundled_path},
                        {"system_path", system_path},
                        {"version", openconnect_version(resolved_path)},
                        {"bundled_runtime_dir", utils::get_bundled_runtime_dir()}};

#ifdef _WIN32
  legacy["wintun_path"] = utils::get_bundled_wintun_path();
  legacy["tap_installer_path"] = utils::get_bundled_tap_installer_path();
#endif

  if (cfg.vpn_engine == "native") {
    nlohmann::json native{{"engine", "native"},
                          {"mode", "native"},
                          {"available", true},
                          {"source", "native"},
                          {"path", ""},
                          {"version", ECNUVPN_VERSION},
                          {"bundled_runtime_dir", utils::get_bundled_runtime_dir()},
                          {"legacy_openconnect", legacy}};
#ifdef _WIN32
    native["wintun_path"] = utils::get_bundled_wintun_path();
    native["tap_installer_path"] = utils::get_bundled_tap_installer_path();
#endif
    return native;
  }

  return legacy;
}

} // namespace platform
} // namespace ecnuvpn
