#pragma once

#include <deque>
#include <functional>
#include <future>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

namespace exv::core::lifecycle {
struct CoreResolverDeps;
}

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
  virtual void close() {}
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
  ~CoreRpcClient();

  CoreRpcResponse invoke(const CoreRpcRequest &request);
  std::future<CoreRpcResponse> invoke_async(CoreRpcRequest request);
  void pump_events();
  void shutdown();
  void set_event_handler(CoreRpcEventHandler handler);

private:
  void ensure_reader_started();
  void reader_loop();
  void resolve_pending(CoreRpcResponse response);
  void resolve_all_pending(CoreRpcResponse response);

  CoreRpcTransport &transport_;
  CoreRpcWireMode wire_mode_ = CoreRpcWireMode::Desktop;
  CoreRpcEventHandler event_handler_;
  std::deque<std::string> buffered_response_lines_;
  std::mutex write_mutex_;
  std::mutex read_mutex_;
  std::mutex pending_mutex_;
  std::mutex event_mutex_;
  std::map<std::string, std::shared_ptr<std::promise<CoreRpcResponse>>> pending_;
  std::map<std::string, CoreRpcResponse> unmatched_responses_;
  std::thread reader_thread_;
  bool reader_started_ = false;
  bool shutting_down_ = false;
};

std::string serialize_core_rpc_request(const CoreRpcRequest &request);
std::string serialize_desktop_rpc_request(const CoreRpcRequest &request);
CoreRpcResponse parse_core_rpc_line(const std::string &line);
CoreRpcEvent parse_core_rpc_event_line(const std::string &line);

// Create CoreResolverDeps that use PipeClient for IPC probe/send.
// This bridges the CLI PipeClient into the shared resolver's DI interface.
exv::core::lifecycle::CoreResolverDeps make_pipe_resolver_deps();

} // namespace ecnuvpn::ui_shell
