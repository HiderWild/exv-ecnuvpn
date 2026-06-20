#pragma once

#include "app/ui_shell/renderer_assets.hpp"

#include <functional>
#include <string>

namespace exv::ui_shell {

using HostMessageHandler = std::function<std::string(const std::string &)>;

struct UiWindowConfig {
  RendererAssets renderer;
  std::string exv_path;
  bool enable_dev_tools = false;
  std::function<void()> pump_core_events;
  std::string state_dir;
};

class UiWindow {
public:
  virtual ~UiWindow() = default;
  virtual void set_message_handler(HostMessageHandler handler) = 0;
  virtual int run(const UiWindowConfig &config) = 0;
  virtual void emit_event(const std::string &event_json) = 0;
  virtual void post_host_response(const std::string &response_json) {
    (void)response_json;
  }
};

} // namespace exv::ui_shell
