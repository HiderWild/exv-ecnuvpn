module;

#include "core/tunnel_controller/reconnect_policy.hpp"

export module exv.core.tunnel.reconnect;

export namespace exv::core::tunnel::reconnect {

using ::exv::core::ClockFunc;
using ::exv::core::DisconnectReason;
using ::exv::core::ErrorInfo;
using ::exv::core::RandomFunc;
using ::exv::core::ReconnectConfig;
using ::exv::core::ReconnectDecision;
using ::exv::core::ReconnectPolicy;
using ::exv::core::TunnelPhase;
using ::exv::core::UserIntent;

} // namespace exv::core::tunnel::reconnect
