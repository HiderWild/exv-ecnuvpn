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

} // namespace ecnuvpn::ui_shell
