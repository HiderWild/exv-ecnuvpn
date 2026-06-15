#include "app/ui_shell/core_rpc_client.hpp"

#include <cassert>

int main() {
  using namespace ecnuvpn::ui_shell;

  CoreRpcResponse ok =
      parse_core_rpc_line(R"({"id":7,"ok":true,"data":{"phase":"idle"}})");
  assert(ok.id == 7);
  assert(ok.ok);
  assert(ok.data_json == R"({"phase":"idle"})");

  CoreRpcResponse error =
      parse_core_rpc_line(R"({"id":8,"ok":false,"code":"bad","message":"No"})");
  assert(error.id == 8);
  assert(!error.ok);
  assert(error.code == "bad");
  assert(error.message == "No");

  CoreRpcEvent event =
      parse_core_rpc_event_line(R"({"event":"status","data":{"phase":"connected"}})");
  assert(event.event == "status");
  assert(event.data_json == R"({"phase":"connected"})");
}
