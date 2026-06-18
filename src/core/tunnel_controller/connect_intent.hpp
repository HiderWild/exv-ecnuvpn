#pragma once

#include <cstdint>
#include <string>

namespace exv::core {

enum class DesiredVpnIntent { Disconnect, Connect };

struct PendingConnectRequest {
  std::string profile_id;
  std::string server;
  bool has_password = false;
};

struct VpnWorkflowIntent {
  DesiredVpnIntent desired = DesiredVpnIntent::Disconnect;
  std::uint64_t epoch = 0;
  PendingConnectRequest pending_connect;
};

} // namespace exv::core
