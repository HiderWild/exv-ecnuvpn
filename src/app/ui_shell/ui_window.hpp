#pragma once

#include "app/ui_shell/renderer_assets.hpp"

#include <functional>
#include <string>

namespace ecnuvpn::ui_shell {

using HostMessageHandler = std::function<std::string(const std::string &)>;

struct UiWindowConfig {
  RendererAssets renderer;
  bool enable_dev_tools = false;
};

class UiWindow {
public:
  virtual ~UiWindow() = default;
  virtual void set_message_handler(HostMessageHandler handler) = 0;
  virtual int run(const UiWindowConfig &config) = 0;
  virtual void emit_event(const std::string &event_json) = 0;
};

} // namespace ecnuvpn::ui_shell
