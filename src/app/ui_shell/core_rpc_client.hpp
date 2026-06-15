#pragma once

#include <deque>
#include <functional>
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

using CoreRpcEventHandler = std::function<void(const CoreRpcEvent &)>;

class CoreRpcTransport {
public:
  virtual ~CoreRpcTransport() = default;
  virtual bool write_line(const std::string &line) = 0;
  virtual bool read_line(std::string &line) = 0;
  virtual bool read_available_line(std::string &line) { return false; }
};

enum class CoreRpcWireMode {
  Desktop,
  Native,
};

class CoreRpcClient {
public:
  // UI shell renderer requests are desktop RPC actions, so the default wire
  // mode preserves the desktop id/action/payload envelope.
  explicit CoreRpcClient(
      CoreRpcTransport &transport,
      CoreRpcWireMode wire_mode = CoreRpcWireMode::Desktop);

  CoreRpcResponse invoke(const CoreRpcRequest &request);
  void pump_events();
  void set_event_handler(CoreRpcEventHandler handler);

private:
  CoreRpcTransport &transport_;
  CoreRpcWireMode wire_mode_ = CoreRpcWireMode::Desktop;
  CoreRpcEventHandler event_handler_;
  std::deque<std::string> buffered_response_lines_;
};

std::string serialize_core_rpc_request(const CoreRpcRequest &request);
std::string serialize_desktop_rpc_request(const CoreRpcRequest &request);
CoreRpcResponse parse_core_rpc_line(const std::string &line);
CoreRpcEvent parse_core_rpc_event_line(const std::string &line);

} // namespace ecnuvpn::ui_shell
