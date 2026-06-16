#include "app/ui_shell/ui_window.hpp"

#include <memory>

namespace ecnuvpn::platform::linux::ui_shell {
std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_webkitgtk_window();
}

int main() {
  auto window = ecnuvpn::platform::linux::ui_shell::create_webkitgtk_window();
  return window ? 0 : 1;
}
