#include "app/ui_shell/ui_shell_runtime.hpp"

#include "app/ui_shell/async_host_bridge.hpp"

#include <nlohmann/json.hpp>

#include <exception>
#include <mutex>
#include <string_view>

namespace ecnuvpn::ui_shell {
namespace {

class WindowMessageHandlerGuard {
public:
  explicit WindowMessageHandlerGuard(UiWindow &window) : window_(window) {}
  ~WindowMessageHandlerGuard() {
    window_.set_message_handler({});
  }

  WindowMessageHandlerGuard(const WindowMessageHandlerGuard &) = delete;
  WindowMessageHandlerGuard &operator=(const WindowMessageHandlerGuard &) = delete;

private:
  UiWindow &window_;
};

class CoreEventHandlerGuard {
public:
  explicit CoreEventHandlerGuard(CoreRpcClient &client) : client_(client) {}
  ~CoreEventHandlerGuard() {
    client_.set_event_handler({});
  }

  CoreEventHandlerGuard(const CoreEventHandlerGuard &) = delete;
  CoreEventHandlerGuard &operator=(const CoreEventHandlerGuard &) = delete;

private:
  CoreRpcClient &client_;
};

int request_id_from_message(const std::string &message_json) {
  try {
    const auto parsed = nlohmann::json::parse(message_json);
    if (parsed.is_object() && parsed.contains("id") && parsed.at("id").is_number_integer()) {
      return parsed.at("id").get<int>();
    }
  } catch (const nlohmann::json::exception &) {
  }
  return 0;
}

std::string callback_error_response(const std::string &message_json,
                                    std::string_view message) {
  nlohmann::ordered_json out;
  out["id"] = request_id_from_message(message_json);
  out["ok"] = false;
  out["code"] = "host_bridge_error";
  out["message"] = message;
  return out.dump();
}

std::string renderer_event_envelope(const CoreRpcEvent &event) {
  nlohmann::ordered_json out;
  out["type"] = event.event;
  out["data"] = event.data_json.empty()
                    ? nlohmann::json::object()
                    : nlohmann::json::parse(event.data_json);
  return out.dump();
}

} // namespace

int run_ui_shell_window(UiWindow &window,
                        const UiWindowConfig &config,
                        CoreRpcClient &client) {
  client.set_event_handler([&window](const CoreRpcEvent &event) {
    window.emit_event(renderer_event_envelope(event));
  });
  CoreEventHandlerGuard event_guard(client);

  UiWindowConfig runtime_config = config;
  std::mutex client_mutex;
  runtime_config.pump_core_events = [&client, &client_mutex]() {
    if (!client_mutex.try_lock()) {
      return;
    }
    std::lock_guard<std::mutex> lock(client_mutex, std::adopt_lock);
    client.pump_events();
  };

  AsyncHostBridge bridge(client, [&window](std::string response_json) {
    window.post_host_response(response_json);
  });

  window.set_message_handler([&bridge](const std::string &message_json) {
    try {
      bridge.accept_message(message_json);
      return accepted_host_response();
    } catch (const std::exception &error) {
      return callback_error_response(message_json, error.what());
    } catch (...) {
      return callback_error_response(message_json, "Unknown host bridge error");
    }
  });
  WindowMessageHandlerGuard handler_guard(window);
  const int exit_code = window.run(runtime_config);
  bridge.shutdown();
  return exit_code;
}

} // namespace ecnuvpn::ui_shell
