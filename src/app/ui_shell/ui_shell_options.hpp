#pragma once

#include <filesystem>
#include <string>

namespace ecnuvpn::ui_shell {

struct UiShellOptions {
  std::string renderer_dev_server_url;
  std::string packaged_renderer_index;
  std::string exv_path;
  bool enable_dev_tools = false;
};

UiShellOptions parse_ui_shell_options(int argc, char **argv);
UiShellOptions parse_ui_shell_args_file(const std::filesystem::path &args_file);
UiShellOptions load_packaged_ui_shell_options(
    const std::filesystem::path &executable_path);
UiShellOptions resolve_ui_shell_options(
    int argc, char **argv, const std::filesystem::path &executable_path);
std::string validate_ui_shell_options(const UiShellOptions &options);

} // namespace ecnuvpn::ui_shell
