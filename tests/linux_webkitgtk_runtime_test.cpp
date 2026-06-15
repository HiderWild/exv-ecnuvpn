#include "app/ui_shell/ui_window.hpp"

namespace ecnuvpn::platform::linux::ui_shell {
int run_webkitgtk_host(const ecnuvpn::ui_shell::UiWindowConfig &config);
}

int main() {
  ecnuvpn::ui_shell::UiWindowConfig config;
  const int result =
      ecnuvpn::platform::linux::ui_shell::run_webkitgtk_host(config);
  return result == 70 ? 0 : 1;
}
