#pragma once

#include <string>

namespace ecnuvpn::ui_shell {

struct CoreRpcRequest {
  std::string action;
  std::string payload_json;
  std::string request_id;
};

struct CoreRpcResponse {
  int id = 0;
  bool ok = false;
  std::string data_json;
  std::string code;
  std::string message;
};

struct CoreRpcEvent {
  std::string event;
  std::string data_json;
};

CoreRpcResponse parse_core_rpc_line(const std::string &line);
CoreRpcEvent parse_core_rpc_event_line(const std::string &line);

} // namespace ecnuvpn::ui_shell
