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
  std::string request_id;
  bool ok = false;
  std::string data_json;
  std::string code;
  std::string message;
};

struct CoreRpcEvent {
  std::string event;
  std::string data_json;
};

class CoreRpcTransport {
public:
  virtual ~CoreRpcTransport() = default;
  virtual bool write_line(const std::string &line) = 0;
  virtual bool read_line(std::string &line) = 0;
};

class CoreRpcClient {
public:
  explicit CoreRpcClient(CoreRpcTransport &transport);

  CoreRpcResponse invoke(const CoreRpcRequest &request);

private:
  CoreRpcTransport &transport_;
};

std::string serialize_core_rpc_request(const CoreRpcRequest &request);
CoreRpcResponse parse_core_rpc_line(const std::string &line);
CoreRpcEvent parse_core_rpc_event_line(const std::string &line);

} // namespace ecnuvpn::ui_shell
