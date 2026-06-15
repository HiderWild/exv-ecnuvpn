#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/ui_shell_runtime.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace {

class FakeTransport final : public ecnuvpn::ui_shell::CoreRpcTransport {
public:
  explicit FakeTransport(std::string response) : response_(std::move(response)) {}

  bool write_line(const std::string &line) override {
    writes.push_back(line);
    return true;
  }

  bool read_line(std::string &line) override {
    line = response_;
    return true;
  }

  std::vector<std::string> writes;

private:
  std::string response_;
};

class FakeWindow final : public ecnuvpn::ui_shell::UiWindow {
public:
  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &config) override {
    observed_config = config;
    if (throw_on_run) {
      throw std::runtime_error("window failed");
    }
    if (!handler_) {
      return 91;
    }
    observed_response = dispatch(message_json);
    return 12;
  }

  void emit_event(const std::string &event_json) override {
    emitted_events.push_back(event_json);
  }

  ecnuvpn::ui_shell::UiWindowConfig observed_config;
  std::string message_json =
      R"({"id":11,"action":"config.getAuth","payload":{"profile":"default"}})";
  bool throw_on_run = false;
  std::string observed_response;
  std::vector<std::string> emitted_events;

  bool has_message_handler() const {
    return static_cast<bool>(handler_);
  }

  std::string dispatch(const std::string &message_json) const {
    if (!handler_) {
      return {};
    }
    return handler_(message_json);
  }

private:
  ecnuvpn::ui_shell::HostMessageHandler handler_;
};

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << "\n";
  return false;
}

} // namespace

int main() {
  using namespace ecnuvpn::ui_shell;
  bool ok = true;

  FakeTransport transport(R"({"id":11,"ok":true,"data":{"username":"alice"}})");
  CoreRpcClient client(transport);
  FakeWindow window;

  UiWindowConfig config;
  config.renderer = resolve_renderer_assets("http://127.0.0.1:8288", "");
  config.enable_dev_tools = true;

  const int exit_code =
      run_ui_shell_window(window, config, client);

  ok = expect(exit_code == 12, "runtime should return platform window exit code") && ok;
  ok = expect(!window.has_message_handler(),
              "runtime should clear the window message handler after run") &&
       ok;
  ok = expect(window.dispatch(R"({"id":99,"action":"status.get","payload":{}})").empty(),
              "cleared window message handler should not dispatch after run") &&
       ok;
  ok = expect(window.observed_config.renderer.location == "http://127.0.0.1:8288",
              "runtime should pass renderer config to window") &&
       ok;
  ok = expect(window.observed_config.enable_dev_tools,
              "runtime should pass devtools flag to window") &&
       ok;
  ok = expect(window.observed_response ==
                  R"({"id":11,"ok":true,"data":{"username":"alice"}})",
              "runtime should route window message through host bridge") &&
       ok;
  ok = expect(transport.writes.size() == 1,
              "runtime should forward one request to core") &&
       ok;
  if (transport.writes.size() == 1) {
    ok = expect(transport.writes[0] ==
                    R"({"id":11,"action":"config.getAuth","payload":{"profile":"default"}})",
                "runtime should preserve desktop RPC envelope") &&
         ok;
  }

  FakeTransport unused_transport(R"({"id":0,"ok":true,"data":{}})");
  CoreRpcClient unused_client(unused_transport);
  FakeWindow bad_message_window;
  bad_message_window.message_json = R"({"id":[],"action":"status.get","payload":{}})";
  const int bad_message_exit =
      run_ui_shell_window(bad_message_window, config, unused_client);
  ok = expect(bad_message_exit == 12,
              "runtime should keep window exit code after callback error") &&
       ok;
  const auto bad_message_response =
      nlohmann::json::parse(bad_message_window.observed_response);
  ok = expect(bad_message_response.value("id", -1) == 0,
              "runtime callback error should keep a numeric id") &&
       ok;
  ok = expect(!bad_message_response.value("ok", true),
              "runtime callback error should set ok=false") &&
       ok;
  ok = expect(bad_message_response.value("code", "") == "host_bridge_error",
              "runtime callback error code should be stable") &&
       ok;
  ok = expect(!bad_message_response.value("message", "").empty(),
              "runtime callback error should include a message") &&
       ok;
  ok = expect(unused_transport.writes.empty(),
              "invalid host request should not reach core transport") &&
       ok;

  FakeTransport throwing_transport(R"({"id":0,"ok":true,"data":{}})");
  CoreRpcClient throwing_client(throwing_transport);
  FakeWindow throwing_window;
  throwing_window.throw_on_run = true;
  bool saw_throw = false;
  try {
    (void)run_ui_shell_window(throwing_window, config, throwing_client);
  } catch (const std::runtime_error &error) {
    saw_throw = std::string(error.what()) == "window failed";
  }
  ok = expect(saw_throw, "runtime should propagate window run exceptions") && ok;
  ok = expect(!throwing_window.has_message_handler(),
              "runtime should clear the message handler when window run throws") &&
       ok;

  return ok ? 0 : 1;
}
