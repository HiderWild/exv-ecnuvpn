#include "app/ui_shell/ui_window.hpp"

namespace ecnuvpn::platform::darwin::ui_shell {
int run_wk_webview_host(const ecnuvpn::ui_shell::UiWindowConfig &config);
}

int main() {
  ecnuvpn::ui_shell::UiWindowConfig config;
  const int result =
      ecnuvpn::platform::darwin::ui_shell::run_wk_webview_host(config);
  return result == 70 ? 0 : 1;
}
