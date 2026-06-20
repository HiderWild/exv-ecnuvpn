#include "helper/common/pipe_helper_client.hpp"
#include "helper/helper.hpp"
#include "platform/common/helper_client.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <string>
#include <thread>
#include <vector>

#include <windows.h>
#include <sddl.h>

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

std::string current_owner_sid() {
  HANDLE token = nullptr;
  if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token))
    return "";

  DWORD needed = 0;
  GetTokenInformation(token, TokenUser, nullptr, 0, &needed);
  if (needed == 0) {
    CloseHandle(token);
    return "";
  }

  std::vector<unsigned char> buffer(needed);
  if (!GetTokenInformation(token, TokenUser, buffer.data(), needed, &needed)) {
    CloseHandle(token);
    return "";
  }
  CloseHandle(token);

  auto *user = reinterpret_cast<TOKEN_USER *>(buffer.data());
  LPSTR sid = nullptr;
  if (!ConvertSidToStringSidA(user->User.Sid, &sid))
    return "";

  std::string result = sid;
  LocalFree(sid);
  return result;
}

exv::helper::DaemonOptions make_oneshot_options(
    const std::string &endpoint, const std::string &owner = std::string()) {
  exv::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.owner = owner.empty() ? current_owner_sid() : owner;
  options.parent_pid = static_cast<int>(GetCurrentProcessId());
  options.oneshot = true;
  return options;
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

  exv::helper::DaemonOptions options = make_oneshot_options(endpoint);

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = exv::helper::daemon_main(options); });

  const exv::platform::HelperEndpoint helper_endpoint{endpoint};

  bool ok = true;
  bool last_request_ok = false;
  for (int i = 0; i < request_count; ++i) {
    const auto hello = exv::platform::send_helper_request(
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
    exv::helper::request_daemon_stop();
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

  exv::helper::DaemonOptions options = make_oneshot_options(endpoint);

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = exv::helper::daemon_main(options); });

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

  exv::helper::AcquireCoreLeaseRequest acquire_req;
  acquire_req.core_pid = static_cast<int>(GetCurrentProcessId());
  acquire_req.purpose = "connect";
  const auto acquired = client.acquire_core_lease(acquire_req);
  ok = expect(acquired.accepted,
              "one-shot helper should acquire a core lease") &&
       ok;
  ok = expect(!acquired.lease_id.empty(),
              "one-shot helper should return a core lease id") &&
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
  ok = expect(!shutdown.exiting,
              "one-shot helper should keep running after VPN shutdown while core lease is active") &&
       ok;

  const auto inspect = client.inspect(exv::helper::InspectRequest{});
  ok = expect(inspect.core_lease.active,
              "one-shot helper should answer Inspect after VPN shutdown") &&
       ok;
  ok = expect(inspect.core_lease.lease_id == acquired.lease_id,
              "Inspect should report the active core lease") &&
       ok;
  ok = expect(!inspect.session_state.active,
              "Inspect should report no active VPN session after Shutdown") &&
       ok;

  exv::helper::ReleaseCoreLeaseRequest release_req;
  release_req.lease_id = acquired.lease_id;
  release_req.exit_if_oneshot = true;
  const auto released = client.release_core_lease(release_req);
  ok = expect(released.released,
              "one-shot helper should release the core lease") &&
       ok;
  ok = expect(released.exiting,
              "ReleaseCoreLease should request one-shot helper exit") &&
       ok;

  client.disconnect();
  daemon.join();
  ok = expect(daemon_rc == 0,
              "one-shot helper should exit with rc=0 after persistent test") &&
       ok;
  return ok;
}

bool oneshot_helper_rejects_first_packet_that_is_not_hello() {
  const std::string endpoint = random_endpoint();

  exv::helper::DaemonOptions options = make_oneshot_options(endpoint);

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = exv::helper::daemon_main(options); });

  const exv::platform::HelperEndpoint helper_endpoint{endpoint};
  exv::helper::StartSessionRequest start_req;
  start_req.profile_id.value = "test-profile";
  exv::helper::HelperRequest request;
  request.op = exv::helper::HelperOp::StartSession;
  request.payload_json = nlohmann::json(start_req).dump();

  const auto response = exv::platform::send_helper_request(
      helper_endpoint, nlohmann::json(request));

  bool ok = true;
  ok = expect(response.is_object(),
              "one-shot helper should return an object for illegal first packet") &&
       ok;
  ok = expect(!response.value("success", true),
              "one-shot helper must reject non-Hello first packet") &&
       ok;
  ok = expect(response.value("error_code", std::string()) == "hello_required",
              "one-shot helper should report hello_required") &&
       ok;

  daemon.join();
  ok = expect(daemon_rc == 0,
              "one-shot helper should exit after illegal first packet") &&
       ok;
  return ok;
}

bool oneshot_helper_rejects_non_owner_client() {
  const std::string endpoint = random_endpoint();

  exv::helper::DaemonOptions options =
      make_oneshot_options(endpoint, "S-1-0-0");

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = exv::helper::daemon_main(options); });

  const auto response = exv::platform::send_helper_request(
      exv::platform::HelperEndpoint{endpoint},
      nlohmann::json(make_hello_request()));

  bool ok = true;
  ok = expect(response.is_object(),
              "one-shot helper should return an object for owner mismatch") &&
       ok;
  ok = expect(!response.value("ok", true),
              "one-shot helper must reject a non-owner client") &&
       ok;
  ok = expect(response.value("code", std::string()) == "permission_denied",
              "one-shot helper should report permission_denied") &&
       ok;

  daemon.join();
  ok = expect(daemon_rc == 0,
              "one-shot helper should exit after owner mismatch") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = oneshot_helper_responds_to_hello_and_exits_after_disconnect() && ok;
  ok = oneshot_helper_preserves_persistent_connection_and_shutdown() && ok;
  ok = oneshot_helper_rejects_first_packet_that_is_not_hello() && ok;
  ok = oneshot_helper_rejects_non_owner_client() && ok;
  return ok ? 0 : 1;
}
