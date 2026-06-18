#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/ui_shell_runtime.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <condition_variable>
#include <future>
#include <iostream>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>
#include <vector>

namespace {

class FakeTransport final : public ecnuvpn::ui_shell::CoreRpcTransport {
public:
  explicit FakeTransport(std::string response) {
    responses_.push_back(std::move(response));
  }

  explicit FakeTransport(std::vector<std::string> responses)
      : responses_(std::move(responses)) {}

  bool write_line(const std::string &line) override {
    writes.push_back(line);
    return true;
  }

  bool read_line(std::string &line) override {
    if (block_reads) {
      std::unique_lock<std::mutex> lock(read_mutex);
      read_cv.wait(lock, [this] { return reads_released; });
    }
    if (next_response_ >= responses_.size()) {
      return false;
    }
    line = responses_[next_response_++];
    return true;
  }

  bool read_available_line(std::string &line) override {
    if (next_available_line_ >= available_lines.size()) {
      return false;
    }
    line = available_lines[next_available_line_++];
    return true;
  }

  std::vector<std::string> writes;
  std::vector<std::string> available_lines;
  bool block_reads = false;
  bool reads_released = false;
  std::mutex read_mutex;
  std::condition_variable read_cv;

  void release_reads() {
    {
      std::lock_guard<std::mutex> lock(read_mutex);
      reads_released = true;
    }
    read_cv.notify_all();
  }

private:
  std::vector<std::string> responses_;
  std::vector<std::string>::size_type next_response_ = 0;
  std::vector<std::string>::size_type next_available_line_ = 0;
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
    if (pump_core_events_before_request && observed_config.pump_core_events) {
      observed_config.pump_core_events();
    }
    if (!handler_) {
      return 91;
    }
    if (on_before_dispatch) {
      on_before_dispatch();
    }
    observed_response = dispatch(message_json);
    return 12;
  }

  void emit_event(const std::string &event_json) override {
    emitted_events.push_back(event_json);
  }

  void post_host_response(const std::string &response_json) override {
    posted_host_responses.push_back(response_json);
  }

  ecnuvpn::ui_shell::UiWindowConfig observed_config;
  std::string message_json =
      R"({"id":11,"action":"config.getAuth","payload":{"profile":"default"}})";
  bool throw_on_run = false;
  bool pump_core_events_before_request = false;
  std::function<void()> on_before_dispatch;
  std::string observed_response;
  std::vector<std::string> emitted_events;
  std::vector<std::string> posted_host_responses;

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
  config.exv_path = "C:/app/bin/exv.exe";
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
  ok = expect(window.observed_config.exv_path == "C:/app/bin/exv.exe",
              "runtime should pass exv path to window") &&
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

  FakeTransport event_transport(
      std::vector<std::string>{R"({"event":"log","data":{"line":"connected"}})",
                               R"({"id":11,"ok":true,"data":{"username":"alice"}})"});
  CoreRpcClient event_client(event_transport);
  FakeWindow event_window;
  const int event_exit_code =
      run_ui_shell_window(event_window, config, event_client);
  ok = expect(event_exit_code == 12,
              "runtime should preserve exit code when forwarding events") &&
       ok;
  ok = expect(event_window.emitted_events.size() == 1,
              "runtime should emit one renderer event") &&
       ok;
  if (event_window.emitted_events.size() == 1) {
    ok = expect(event_window.emitted_events[0] ==
                    R"({"type":"log","data":{"line":"connected"}})",
                "runtime should map core event to renderer envelope") &&
         ok;
  }

  FakeTransport unsolicited_event_transport(
      R"({"id":11,"ok":true,"data":{"username":"alice"}})");
  unsolicited_event_transport.available_lines.push_back(
      R"({"event":"vpn.connected","data":{"profile":"default"}})");
  CoreRpcClient unsolicited_event_client(unsolicited_event_transport);
  FakeWindow unsolicited_event_window;
  unsolicited_event_window.pump_core_events_before_request = true;
  const int unsolicited_event_exit_code =
      run_ui_shell_window(unsolicited_event_window, config,
                          unsolicited_event_client);
  ok = expect(unsolicited_event_exit_code == 12,
              "runtime should preserve exit code when pumping events") &&
       ok;
  ok = expect(unsolicited_event_window.emitted_events.size() == 1,
              "runtime should emit unsolicited core event before renderer request") &&
       ok;
  if (unsolicited_event_window.emitted_events.size() == 1) {
    ok = expect(unsolicited_event_window.emitted_events[0] ==
                    R"({"type":"vpn.connected","data":{"profile":"default"}})",
                "runtime should map pumped core event to renderer envelope") &&
         ok;
  }
  ok = expect(unsolicited_event_transport.writes.size() == 1,
              "runtime should still forward renderer request after pumping events") &&
       ok;

  {
    FakeTransport delayed_transport(
        R"({"id":44,"ok":true,"data":{"items":[]}})");
    delayed_transport.block_reads = true;
    CoreRpcClient delayed_client(delayed_transport);
    FakeWindow delayed_window;
    delayed_window.message_json =
        R"({"id":44,"action":"logs.list","payload":{}})";

    std::promise<void> run_entered;
    std::future<void> run_entered_future = run_entered.get_future();
    delayed_window.on_before_dispatch = [&run_entered]() {
      run_entered.set_value();
    };

    std::future<int> run_future = std::async(std::launch::async, [&]() {
      return run_ui_shell_window(delayed_window, config, delayed_client);
    });

    ok = expect(run_entered_future.wait_for(std::chrono::milliseconds(500)) ==
                    std::future_status::ready,
                "runtime non-blocking test should enter window dispatch") &&
         ok;
    ok = expect(run_future.wait_for(std::chrono::milliseconds(100)) ==
                    std::future_status::ready,
                "host message dispatch should return before core response is available") &&
         ok;
    ok = expect(delayed_window.observed_response.empty(),
                "async host bridge should not return a synchronous core response") &&
         ok;
    delayed_transport.release_reads();
    ok = expect(run_future.get() == 12,
                "runtime should preserve window exit code after async dispatch") &&
         ok;
    ok = expect(delayed_window.posted_host_responses.size() == 1,
                "async host bridge should post the eventual core response") &&
         ok;
    if (delayed_window.posted_host_responses.size() == 1) {
      ok = expect(delayed_window.posted_host_responses[0] ==
                      R"({"id":44,"ok":true,"data":{"items":[]}})",
                  "async host bridge should post response by original id") &&
           ok;
    }
  }

  FakeTransport empty_event_data_transport(
      std::vector<std::string>{R"({"event":"heartbeat"})",
                               R"({"id":11,"ok":true,"data":{"username":"alice"}})"});
  CoreRpcClient empty_event_data_client(empty_event_data_transport);
  FakeWindow empty_event_data_window;
  (void)run_ui_shell_window(empty_event_data_window, config,
                            empty_event_data_client);
  ok = expect(empty_event_data_window.emitted_events.size() == 1,
              "runtime should emit event without data") &&
       ok;
  if (empty_event_data_window.emitted_events.size() == 1) {
    ok = expect(empty_event_data_window.emitted_events[0] ==
                    R"({"type":"heartbeat","data":{}})",
                "runtime should map missing event data to empty object") &&
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
