#include "app/ui_shell/ui_shell_runtime.hpp"

#include <nlohmann/json.hpp>

#include <exception>
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

} // namespace

int run_ui_shell_window(UiWindow &window,
                        const UiWindowConfig &config,
                        CoreRpcClient &client) {
  window.set_message_handler([&client](const std::string &message_json) {
    try {
      return handle_host_request(message_json, [&client](const CoreRpcRequest &request) {
        return client.invoke(request);
      });
    } catch (const std::exception &error) {
      return callback_error_response(message_json, error.what());
    } catch (...) {
      return callback_error_response(message_json, "Unknown host bridge error");
    }
  });
  WindowMessageHandlerGuard handler_guard(window);
  return window.run(config);
}

} // namespace ecnuvpn::ui_shell
