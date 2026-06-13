#include "helper_common/pipe_helper_client.hpp"
#include "helper.hpp"
#include "platform/common/helper_client.hpp"

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
  if (value.value("ok", false))
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

bool run_oneshot_daemon_hello_sequence(const char *token,
                                       int request_count,
                                       const char *description) {
  const std::string endpoint = random_endpoint();

  ecnuvpn::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.auth_token = token;
  options.auth_required = true;
  options.oneshot = true;

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = ecnuvpn::helper::daemon_main(options); });

  const ecnuvpn::platform::HelperEndpoint helper_endpoint{endpoint,
                                                          options.auth_token};

  bool ok = true;
  bool last_request_ok = false;
  for (int i = 0; i < request_count; ++i) {
    const auto hello = ecnuvpn::platform::send_helper_request(
        helper_endpoint, {{"action", "hello"}});
    last_request_ok = hello.value("ok", false);
    ok = expect_json_ok(hello, description) && ok;
    ok = expect(hello.value("mode", std::string()) == "oneshot",
                "one-shot helper hello should report mode=oneshot") &&
         ok;
    ok = expect(hello.contains("capabilities") &&
                    hello["capabilities"].value("oneshot_mode", false),
                "one-shot helper hello should report capabilities.oneshot_mode=true") &&
         ok;
  }

  ecnuvpn::helper::request_daemon_stop();
  if (last_request_ok) {
    (void)ecnuvpn::platform::send_helper_request(helper_endpoint,
                                                 {{"action", "hello"}});
  } else {
    wake_acceptor_once(endpoint);
  }

  daemon.join();
  ok = expect(daemon_rc == 0, "one-shot helper should exit with rc=0") && ok;
  return ok;
}

bool oneshot_helper_responds_to_hello() {
  return run_oneshot_daemon_hello_sequence(
      "oneshot-test-token", 1,
      "one-shot helper should answer hello after startup");
}

bool oneshot_helper_accepts_second_connection_after_hello() {
  return run_oneshot_daemon_hello_sequence(
      "oneshot-second-hello-token", 2,
      "one-shot helper should accept a fresh connection after responding to hello");
}

bool oneshot_helper_preserves_v2_persistent_connection() {
  const std::string endpoint = random_endpoint();

  ecnuvpn::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.auth_token = "oneshot-v2-persistent-token";
  options.auth_required = true;
  options.oneshot = true;

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = ecnuvpn::helper::daemon_main(options); });

  exv::helper::PipeClientConfig config;
  config.pipe_path = endpoint;
  config.connect_timeout_ms = 3000;
  exv::helper::PipeHelperClient client(config);

  bool ok = true;
  ok = expect(client.connect(),
              "V2 PipeHelperClient should connect to one-shot helper") &&
       ok;

  exv::helper::HelloRequest req;
  req.client_version = exv::helper::PROTOCOL_VERSION;
  const auto first = client.hello(req);
  ok = expect(first.server_version == exv::helper::PROTOCOL_VERSION,
              "one-shot helper should answer first V2 hello") &&
       ok;

  const auto second = client.hello(req);
  ok = expect(second.server_version == exv::helper::PROTOCOL_VERSION,
              "one-shot helper should answer second V2 hello on same connection") &&
       ok;

  client.disconnect();
  ecnuvpn::helper::request_daemon_stop();
  wake_acceptor_once(endpoint);
  daemon.join();
  ok = expect(daemon_rc == 0,
              "one-shot helper should exit with rc=0 after V2 persistent test") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = oneshot_helper_responds_to_hello() && ok;
  ok = oneshot_helper_accepts_second_connection_after_hello() && ok;
  ok = oneshot_helper_preserves_v2_persistent_connection() && ok;
  return ok ? 0 : 1;
}
