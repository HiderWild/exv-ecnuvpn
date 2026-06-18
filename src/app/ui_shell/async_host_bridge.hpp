#pragma once

#include "app/ui_shell/core_rpc_client.hpp"

#include <atomic>
#include <functional>
#include <memory>
#include <string>

namespace ecnuvpn::ui_shell {

using HostResponsePoster = std::function<void(std::string)>;

class AsyncHostBridge {
public:
  AsyncHostBridge(CoreRpcClient &client, HostResponsePoster post_response);
  ~AsyncHostBridge();

  bool accept_message(std::string message_json);
  void shutdown();

private:
  CoreRpcClient &client_;
  HostResponsePoster post_response_;
  std::shared_ptr<std::atomic<bool>> stopped_;
};

std::string accepted_host_response();

} // namespace ecnuvpn::ui_shell
