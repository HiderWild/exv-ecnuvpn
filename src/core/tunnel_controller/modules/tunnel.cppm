module;

#include "core/tunnel_controller/reconnect_policy.hpp"
#include "core/tunnel_controller/tunnel_controller_fwd.hpp"
#include "core/tunnel_controller/tunnel_events.hpp"
#include "core/tunnel_controller/tunnel_intent.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"

export module exv.core.tunnel;

export namespace exv::core::tunnel {

using ::exv::core::DisconnectReason;
using ::exv::core::ErrorInfo;
using ::exv::core::ProfileId;
using ::exv::core::ReconnectConfig;
using ::exv::core::ReconnectDecision;
using ::exv::core::ReconnectInfo;
using ::exv::core::TunnelController;
using ::exv::core::TunnelEvent;
using ::exv::core::TunnelEventType;
using ::exv::core::TunnelPhase;
using ::exv::core::TunnelStatusSnapshot;
using ::exv::core::UserIntent;

} // namespace exv::core::tunnel
