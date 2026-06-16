#include "app/ui_shell/ui_window.hpp"

#include <memory>
#include <string>
#include <utility>

namespace ecnuvpn::platform::darwin::ui_shell {

namespace {

class WkWebViewWindow final : public ecnuvpn::ui_shell::UiWindow {
public:
  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &) override {
    return 70;
  }

  void emit_event(const std::string &event_json) override {
    last_event_json_ = event_json;
  }

private:
  ecnuvpn::ui_shell::HostMessageHandler handler_;
  std::string last_event_json_;
};

} // namespace

std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_wk_webview_window() {
  return std::make_unique<WkWebViewWindow>();
}

} // namespace ecnuvpn::platform::darwin::ui_shell
