#include "app/ui_shell/ui_window.hpp"

#include <memory>

namespace ecnuvpn::platform::darwin::ui_shell {
std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_wk_webview_window();
}

int main() {
  auto window = ecnuvpn::platform::darwin::ui_shell::create_wk_webview_window();
  return window ? 0 : 1;
}
