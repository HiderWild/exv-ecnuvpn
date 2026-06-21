#pragma once

#include "app/ui_shell/core_rpc_client.hpp"

#include <atomic>
#include <chrono>
#include <functional>
#include <memory>
#include <string>

namespace exv::ui_shell {

using HostResponsePoster = std::function<void(std::string)>;

class AsyncHostBridge {
public:
  AsyncHostBridge(CoreRpcClient &client, HostResponsePoster post_response,
                  std::chrono::milliseconds request_timeout =
                      std::chrono::seconds(15));
  ~AsyncHostBridge();

  bool accept_message(std::string message_json);
  void shutdown();

private:
  CoreRpcClient &client_;
  HostResponsePoster post_response_;
  std::shared_ptr<std::atomic<bool>> stopped_;
  std::chrono::milliseconds request_timeout_;
};

std::string accepted_host_response();

} // namespace exv::ui_shell
