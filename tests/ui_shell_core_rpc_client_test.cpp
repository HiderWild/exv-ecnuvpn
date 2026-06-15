#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/core_process_manager.hpp"

#include <iostream>
#include <string>
#include <utility>
#include <vector>

namespace {

class FakeTransport final : public ecnuvpn::ui_shell::CoreRpcTransport {
public:
  explicit FakeTransport(std::string response) : response_(std::move(response)) {}

  bool write_line(const std::string &line) override {
    writes.push_back(line);
    return write_ok;
  }

  bool read_line(std::string &line) override {
    if (!read_ok) {
      return false;
    }
    line = response_;
    return true;
  }

  bool write_ok = true;
  bool read_ok = true;
  std::vector<std::string> writes;

private:
  std::string response_;
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
  bool ok_all = true;

  CoreRpcResponse ok =
      parse_core_rpc_line(R"({"id":7,"ok":true,"data":{"phase":"idle"}})");
  ok_all = expect(ok.id == 7, "desktop response id should parse") && ok_all;
  ok_all = expect(ok.ok, "desktop response ok should parse") && ok_all;
  ok_all = expect(ok.data_json == R"({"phase":"idle"})",
                  "desktop response data should parse") &&
           ok_all;

  CoreRpcResponse error =
      parse_core_rpc_line(R"({"id":8,"ok":false,"code":"bad","message":"No"})");
  ok_all = expect(error.id == 8, "desktop error id should parse") && ok_all;
  ok_all = expect(!error.ok, "desktop error ok should parse") && ok_all;
  ok_all = expect(error.code == "bad", "desktop error code should parse") &&
           ok_all;
  ok_all = expect(error.message == "No",
                  "desktop error message should parse") &&
           ok_all;

  CoreRpcEvent event =
      parse_core_rpc_event_line(R"({"event":"status","data":{"phase":"connected"}})");
  ok_all = expect(event.event == "status", "event type should parse") &&
           ok_all;
  ok_all = expect(event.data_json == R"({"phase":"connected"})",
                  "event data should parse") &&
           ok_all;

  CoreRpcRequest request;
  request.action = "status.get";
  request.payload_json = R"({"source":"ui"})";
  request.request_id = "42";
  ok_all = expect(serialize_core_rpc_request(request) ==
                      R"({"action":"status.get","payload_json":"{\"source\":\"ui\"}","request_id":"42"})",
                  "native request should serialize deterministically") &&
           ok_all;

  CoreRpcRequest desktop_request;
  desktop_request.action = "config.getAuth";
  desktop_request.payload_json = R"({"profile":"default"})";
  desktop_request.request_id = "7";
  ok_all = expect(serialize_desktop_rpc_request(desktop_request) ==
                      R"({"id":7,"action":"config.getAuth","payload":{"profile":"default"}})",
                  "desktop request should serialize deterministically") &&
           ok_all;

  CoreRpcRequest empty_payload_request;
  empty_payload_request.action = "status.get";
  empty_payload_request.request_id = "9";
  ok_all =
      expect(serialize_desktop_rpc_request(empty_payload_request) ==
                 R"({"id":9,"action":"status.get","payload":{}})",
             "empty desktop payload should serialize as an object") &&
      ok_all;

  CoreRpcResponse native_ok =
      parse_core_rpc_line(R"({"request_id":"42","success":true,"payload_json":"{\"phase\":\"idle\"}"})");
  ok_all = expect(native_ok.id == 42, "native response id should parse") &&
           ok_all;
  ok_all = expect(native_ok.ok, "native response success should map to ok") &&
           ok_all;
  ok_all = expect(native_ok.data_json == R"({"phase":"idle"})",
                  "native payload_json should map to data_json") &&
           ok_all;

  CoreRpcResponse native_error =
      parse_core_rpc_line(R"({"request_id":"43","success":false,"error_code":"bad","error_message":"No"})");
  ok_all = expect(native_error.id == 43, "native error id should parse") &&
           ok_all;
  ok_all = expect(!native_error.ok, "native error success should map to ok") &&
           ok_all;
  ok_all = expect(native_error.code == "bad",
                  "native error code should parse") &&
           ok_all;
  ok_all = expect(native_error.message == "No",
                  "native error message should parse") &&
           ok_all;

  CoreRpcResponse non_numeric_id =
      parse_core_rpc_line(R"({"request_id":"not-a-number","success":true,"payload_json":"{}"})");
  ok_all = expect(non_numeric_id.id == 0,
                  "non-numeric native request_id should map to id 0") &&
           ok_all;
  ok_all = expect(non_numeric_id.request_id == "not-a-number",
                  "non-numeric native request_id should be preserved") &&
           ok_all;

  CoreRpcResponse overflow_id =
      parse_core_rpc_line(R"({"request_id":"999999999999999999999999","success":true,"payload_json":"{}"})");
  ok_all = expect(overflow_id.id == 0,
                  "overflow native request_id should map to id 0") &&
           ok_all;

  FakeTransport transport(
      R"({"request_id":"42","success":true,"payload_json":"{\"phase\":\"idle\"}"})");
  CoreRpcClient client(transport, CoreRpcWireMode::Native);
  CoreRpcResponse invoked = client.invoke(request);
  ok_all = expect(invoked.ok, "client invoke should succeed") && ok_all;
  ok_all = expect(invoked.data_json == R"({"phase":"idle"})",
                  "client invoke should return data") &&
           ok_all;
  ok_all = expect(transport.writes.size() == 1,
                  "client should write exactly one request") &&
           ok_all;
  if (transport.writes.size() == 1) {
    ok_all = expect(transport.writes[0] == serialize_core_rpc_request(request),
                    "client should write serialized native request") &&
             ok_all;
  }

  FakeTransport desktop_transport(
      R"({"id":7,"ok":true,"data":{"username":"alice"}})");
  CoreRpcClient desktop_client(desktop_transport);
  CoreRpcResponse desktop_invoked = desktop_client.invoke(desktop_request);
  ok_all = expect(desktop_invoked.ok, "desktop client invoke should succeed") &&
           ok_all;
  ok_all = expect(desktop_invoked.data_json == R"({"username":"alice"})",
                  "desktop client invoke should return desktop data") &&
           ok_all;
  ok_all = expect(desktop_transport.writes.size() == 1,
                  "desktop client should write exactly one request") &&
           ok_all;
  if (desktop_transport.writes.size() == 1) {
    ok_all =
        expect(desktop_transport.writes[0] ==
                   serialize_desktop_rpc_request(desktop_request),
               "default CoreRpcClient constructor should use desktop envelope") &&
        ok_all;
  }

  FakeTransport empty_payload_transport(
      R"({"id":9,"ok":true,"data":{"phase":"idle"}})");
  CoreRpcResponse empty_payload_invoked =
      CoreRpcClient(empty_payload_transport).invoke(empty_payload_request);
  ok_all = expect(empty_payload_invoked.ok,
                  "empty payload desktop invoke should succeed") &&
           ok_all;
  ok_all = expect(empty_payload_transport.writes.size() == 1,
                  "empty payload desktop invoke should write once") &&
           ok_all;
  if (empty_payload_transport.writes.size() == 1) {
    ok_all =
        expect(empty_payload_transport.writes[0] ==
                   serialize_desktop_rpc_request(empty_payload_request),
               "empty payload desktop invoke should write object payload") &&
        ok_all;
  }

  CoreRpcRequest malformed_payload_request;
  malformed_payload_request.action = "config.getAuth";
  malformed_payload_request.payload_json = "{";
  malformed_payload_request.request_id = "10";
  FakeTransport malformed_payload_transport("");
  CoreRpcResponse malformed_payload_response =
      CoreRpcClient(malformed_payload_transport).invoke(malformed_payload_request);
  ok_all = expect(!malformed_payload_response.ok,
                  "malformed desktop payload should return error response") &&
           ok_all;
  ok_all = expect(malformed_payload_response.code == "protocol_error",
                  "malformed desktop payload error code should be stable") &&
           ok_all;
  ok_all = expect(malformed_payload_transport.writes.empty(),
                  "malformed desktop payload should not write to transport") &&
           ok_all;

  FakeTransport write_failed("");
  write_failed.write_ok = false;
  CoreRpcResponse write_failed_response =
      CoreRpcClient(write_failed).invoke(request);
  ok_all = expect(!write_failed_response.ok,
                  "write failure should return error response") &&
           ok_all;
  ok_all = expect(write_failed_response.code == "transport_closed",
                  "write failure error code should be stable") &&
           ok_all;

  FakeTransport closed("");
  closed.read_ok = false;
  CoreRpcResponse closed_response = CoreRpcClient(closed).invoke(request);
  ok_all = expect(!closed_response.ok,
                  "closed transport should return error response") &&
           ok_all;
  ok_all = expect(closed_response.code == "transport_closed",
                  "closed transport error code should be stable") &&
           ok_all;

  FakeTransport bad_json("not-json");
  CoreRpcResponse bad_json_response = CoreRpcClient(bad_json).invoke(request);
  ok_all = expect(!bad_json_response.ok,
                  "bad JSON response should return error response") &&
           ok_all;
  ok_all = expect(bad_json_response.code == "protocol_error",
                  "bad JSON response error code should be stable") &&
           ok_all;

  FakeTransport wrong_type(R"({"request_id":[],"success":true})");
  CoreRpcResponse wrong_type_response =
      CoreRpcClient(wrong_type).invoke(request);
  ok_all = expect(!wrong_type_response.ok,
                  "wrong response field type should return error response") &&
           ok_all;
  ok_all = expect(wrong_type_response.code == "protocol_error",
                  "wrong response field type error code should be stable") &&
           ok_all;

  CoreProcessLaunch launch;
  launch.state_dir = "C:/state";
  launch.runtime_dir = "C:/runtime";
  std::vector<std::string> args = build_core_process_arguments(launch);
  ok_all = expect((args == std::vector<std::string>{"--mode=core",
                                                    "--config-dir", "C:/state",
                                                    "--home", "C:/runtime"}),
                  "core process stdin arguments should be stable") &&
           ok_all;

  launch.use_stdin = false;
  args = build_core_process_arguments(launch);
  ok_all = expect(args.back() == "--daemon",
                  "daemon core process arguments should include --daemon") &&
           ok_all;

  return ok_all ? 0 : 1;
}
