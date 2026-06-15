#include "app/ui_shell/renderer_assets.hpp"
#include "app/ui_shell/ui_shell_options.hpp"
#include "app/ui_shell/ui_window.hpp"

#include <iostream>

#if defined(_WIN32)
namespace ecnuvpn::platform::win32::ui_shell {
int run_webview2_host(const ecnuvpn::ui_shell::UiWindowConfig &config);
}
#elif defined(__APPLE__)
namespace ecnuvpn::platform::darwin::ui_shell {
int run_wk_webview_host(const ecnuvpn::ui_shell::UiWindowConfig &config);
}
#elif defined(__linux__)
namespace ecnuvpn::platform::linux::ui_shell {
int run_webkitgtk_host(const ecnuvpn::ui_shell::UiWindowConfig &config);
}
#endif

int main(int argc, char **argv) {
  const auto options = ecnuvpn::ui_shell::parse_ui_shell_options(argc, argv);
  const std::string validation_error =
      ecnuvpn::ui_shell::validate_ui_shell_options(options);
  if (!validation_error.empty()) {
    std::cerr << "exv-ui: " << validation_error << '\n';
    return 64;
  }

  ecnuvpn::ui_shell::UiWindowConfig config{
      ecnuvpn::ui_shell::resolve_renderer_assets(
          options.renderer_dev_server_url, options.packaged_renderer_index),
      options.enable_dev_tools,
  };

#if defined(_WIN32)
  return ecnuvpn::platform::win32::ui_shell::run_webview2_host(config);
#elif defined(__APPLE__)
  return ecnuvpn::platform::darwin::ui_shell::run_wk_webview_host(config);
#elif defined(__linux__)
  return ecnuvpn::platform::linux::ui_shell::run_webkitgtk_host(config);
#else
  return 70;
#endif
}
