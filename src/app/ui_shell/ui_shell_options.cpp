#include "app/ui_shell/ui_shell_options.hpp"

#include <string_view>

namespace ecnuvpn::ui_shell {

UiShellOptions parse_ui_shell_options(int argc, char **argv) {
  UiShellOptions options;
  for (int i = 1; i < argc; ++i) {
    std::string_view arg = argv[i] ? argv[i] : "";
    if (arg == "--renderer-url" && i + 1 < argc) {
      options.renderer_dev_server_url = argv[++i];
    } else if (arg == "--renderer-index" && i + 1 < argc) {
      options.packaged_renderer_index = argv[++i];
    } else if (arg == "--exv" && i + 1 < argc) {
      options.exv_path = argv[++i];
    } else if (arg == "--devtools") {
      options.enable_dev_tools = true;
    }
  }
  return options;
}

std::string validate_ui_shell_options(const UiShellOptions &options) {
  if (options.exv_path.empty()) {
    return "missing required --exv path";
  }
  if (options.renderer_dev_server_url.empty() &&
      options.packaged_renderer_index.empty()) {
    return "missing required renderer URL or index path";
  }
  if (!options.renderer_dev_server_url.empty() &&
      !options.packaged_renderer_index.empty()) {
    return "choose either --renderer-url or --renderer-index, not both";
  }
  return {};
}

} // namespace ecnuvpn::ui_shell
