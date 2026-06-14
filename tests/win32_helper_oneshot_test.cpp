#include "helper/common/pipe_helper_client.hpp"
#include "helper/helper.hpp"
#include "helper/platform/helper_client.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <string>
#include <thread>

#include <windows.h>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

bool expect_json_ok(const nlohmann::json &value, const char *message) {
  if (!value.is_object()) {
    std::cerr << "DETAIL: " << value.dump() << std::endl;
    return expect(false, message);
  }
  if (value.value("success", false))
    return true;

  std::cerr << "DETAIL: " << value.dump() << std::endl;
  return expect(false, message);
}

std::string random_endpoint() {
  return "\\\\.\\pipe\\exv-oneshot-test-" +
         std::to_string(GetCurrentProcessId()) + "-" +
         std::to_string(GetTickCount64());
}

void wake_acceptor_once(const std::string &endpoint) {
  HANDLE pipe = CreateFileA(endpoint.c_str(), GENERIC_READ | GENERIC_WRITE, 0,
                            NULL, OPEN_EXISTING, 0, NULL);
  if (pipe != INVALID_HANDLE_VALUE) {
    CloseHandle(pipe);
  }
}

exv::helper::HelperRequest make_hello_request() {
  exv::helper::HelperRequest request;
  request.op = exv::helper::HelperOp::Hello;
  request.payload_json = nlohmann::json(exv::helper::HelloRequest{}).dump();
  return request;
}

bool run_oneshot_daemon_hello_sequence(int request_count,
                                       const char *description) {
  const std::string endpoint = random_endpoint();

  ecnuvpn::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.owner = "test-owner";
  options.parent_pid = static_cast<int>(GetCurrentProcessId());
  options.oneshot = true;

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = ecnuvpn::helper::daemon_main(options); });

  const ecnuvpn::platform::HelperEndpoint helper_endpoint{endpoint};

  bool ok = true;
  bool last_request_ok = false;
  for (int i = 0; i < request_count; ++i) {
    const auto hello = ecnuvpn::platform::send_helper_request(
        helper_endpoint, nlohmann::json(make_hello_request()));
    last_request_ok = hello.value("success", false);
    ok = expect_json_ok(hello, description) && ok;
    const auto payload = nlohmann::json::parse(hello.value("payload_json", "{}"));
    ok = expect(payload["startup_context"].value("launch_mode", std::string()) ==
                    "oneshot",
                "one-shot helper hello should report launch_mode=oneshot") && ok;
    ok = expect(payload.contains("capabilities"),
                "one-shot helper hello should report capabilities") && ok;
  }

  if (!last_request_ok) {
    ecnuvpn::helper::request_daemon_stop();
    wake_acceptor_once(endpoint);
  }

  daemon.join();
  ok = expect(daemon_rc == 0, "one-shot helper should exit with rc=0") && ok;
  return ok;
}

bool oneshot_helper_responds_to_hello_and_exits_after_disconnect() {
  return run_oneshot_daemon_hello_sequence(
      1,
      "one-shot helper should answer hello and exit after connection close");
}

bool oneshot_helper_preserves_persistent_connection_and_shutdown() {
  const std::string endpoint = random_endpoint();

  ecnuvpn::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.owner = "test-owner";
  options.parent_pid = static_cast<int>(GetCurrentProcessId());
  options.oneshot = true;

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = ecnuvpn::helper::daemon_main(options); });

  exv::helper::PipeClientConfig config;
  config.pipe_path = endpoint;
  config.connect_timeout_ms = 3000;
  exv::helper::PipeHelperClient client(config);

  bool ok = true;
  ok = expect(client.connect(),
              "PipeHelperClient should connect to one-shot helper") &&
       ok;

  exv::helper::HelloRequest req;
  const auto first = client.hello(req);
  ok = expect(!first.capabilities.empty(),
              "one-shot helper should answer first hello") &&
       ok;

  const auto second = client.hello(req);
  ok = expect(!second.capabilities.empty(),
              "one-shot helper should answer second hello on same connection") &&
       ok;

  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "test-profile";
  const auto start = client.start_session(start_req);
  ok = expect(!start.session_id.value.empty(),
              "one-shot helper should start a session") &&
       ok;

  exv::helper::ShutdownRequest shutdown_req;
  shutdown_req.session_id = start.session_id;
  shutdown_req.policy.remove_adapter = true;
  const auto shutdown = client.shutdown(shutdown_req);
  ok = expect(shutdown.cleanup_success,
              "one-shot helper should accept active shutdown") &&
       ok;

  client.disconnect();
  daemon.join();
  ok = expect(daemon_rc == 0,
              "one-shot helper should exit with rc=0 after persistent test") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = oneshot_helper_responds_to_hello_and_exits_after_disconnect() && ok;
  ok = oneshot_helper_preserves_persistent_connection_and_shutdown() && ok;
  return ok ? 0 : 1;
}
