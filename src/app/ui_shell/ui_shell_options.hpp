#pragma once

#include <string>

namespace ecnuvpn::ui_shell {

struct UiShellOptions {
  std::string renderer_dev_server_url;
  std::string packaged_renderer_index;
  std::string exv_path;
  bool enable_dev_tools = false;
};

UiShellOptions parse_ui_shell_options(int argc, char **argv);
std::string validate_ui_shell_options(const UiShellOptions &options);

} // namespace ecnuvpn::ui_shell
