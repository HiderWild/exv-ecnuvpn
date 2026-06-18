#pragma once

#include "core/rpc/app_rpc_dispatcher.hpp"

namespace exv::core_api {

class LogActions {
public:
  void register_handlers(AppRpcDispatcher &dispatcher);

  RpcResponse list(const RpcRequest &req);
  RpcResponse clear(const RpcRequest &req);
};

} // namespace exv::core_api
