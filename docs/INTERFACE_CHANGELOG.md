# Interface Changelog

All shared interface changes must be logged here before implementation.
Other agents review this file before starting work.

---

## 2026-06-02: Phase 0 Interface Freeze

### New files created

**Core lifecycle types (`src/core/`):**
- `tunnel_controller.hpp` - TunnelController class (connection lifecycle owner)
- `tunnel_intent.hpp` - UserIntent, ProfileId, DisconnectReason
- `tunnel_state.hpp` - TunnelPhase, ErrorInfo, ReconnectInfo, TunnelStatusSnapshot
- `tunnel_events.hpp` - TunnelEventType, TunnelEvent
- `reconnect_policy.hpp` - ReconnectPolicy, ReconnectDecision, ReconnectConfig
- `core_status_snapshot.hpp` - (re-exports TunnelStatusSnapshot from tunnel_state.hpp)
- `core_error_mapper.hpp` - CoreErrorMapper (maps platform/protocol errors to ErrorInfo)
- `timing.hpp` - StageTimer, ConnectTiming

**Helper protocol V2 (`src/helper_common/`):**
- `helper_protocol.hpp` - Protocol version, HelperOp enum, HelperMode enum
- `helper_messages.hpp` - All request/response message types
- `helper_capabilities.hpp` - Capability constants
- `helper_error.hpp` - HelperError, helper error codes
- `helper_client.hpp` - HelperClient abstract interface
- `helper_connector.hpp` - Platform-specific helper connection factory
- `helper_session_lease.hpp` - SessionId, SessionLease

**Helper runtime (`src/helper_runtime/`):**
- `helper_server.hpp` - HelperServer (listens for Core connections)
- `helper_request_dispatcher.hpp` - Dispatches V2 ops to handlers
- `helper_lifecycle_policy.hpp` - Transient vs Resident lifecycle rules
- `session_lease_manager.hpp` - Manages active session leases
- `cleanup_registry.hpp` - Tracks OS artifacts for crash-safe cleanup
- `helper_session_store.hpp` - Persistent session state

**Platform abstractions (`src/platform/common/`):**
- `platform_network_ops.hpp` - PlatformNetworkOps abstract interface
- `tunnel_device_descriptor.hpp` - TunnelDeviceDescriptor struct
- `packet_device_factory.hpp` - Creates platform-specific PacketDevice
- `route_model.hpp` - Route entry model
- `dns_model.hpp` - DNS configuration model
- `credential_store.hpp` - Secure credential storage interface
- `process_supervision.hpp` - Process lifecycle management
- `ipc_transport.hpp` - Named pipe / Unix socket / XPC abstraction

**RPC dispatcher (`src/core_api/`):**
- `app_rpc_dispatcher.hpp` - Central action router
- `vpn_actions.hpp` - vpn.connect, vpn.disconnect, vpn.get_status
- `config_actions.hpp` - config.* actions
- `service_actions.hpp` - service.* actions
- `route_actions.hpp` - route.* actions

**Error contract (`src/feedback/`):**
- `error_contract.hpp` - ErrorInfo (mirrors core::ErrorInfo for UI serialization)

**Test infrastructure (`tests/support/`):**
- `fake_helper.hpp` / `fake_helper.cpp` - Fake HelperClient for unit tests
- `fake_platform_network_ops.hpp` / `fake_platform_network_ops.cpp` - Fake PlatformNetworkOps
- `fake_core_ui_client.hpp` / `fake_core_ui_client.cpp` - Status snapshot recorder

### Key types

- `exv::core::UserIntent` - Replaces `retry_limit`. UI expresses desired state, Core computes strategy.
- `exv::core::TunnelPhase` - Unified 11-state connection lifecycle.
- `exv::core::ErrorInfo` - Structured error with domain, code, recoverability, recommended action.
- `exv::helper::HelperOp` - V2 protocol operations (Hello, StartSession, ..., EndSession).
- `exv::platform::PlatformNetworkOps` - Control-plane abstraction for privileged network operations.
- `exv::feedback::ErrorInfo` - Error contract for UI serialization (mirrors core::ErrorInfo).

### Breaking changes

None - all new files, no existing files modified.

### Documentation added

- `docs/HELPER_PROTOCOL_V2.md` - Helper V2 protocol specification
- `docs/CORE_STATE_MACHINE.md` - TunnelController state machine specification
- `docs/DESKTOP_RPC_V2.md` - Desktop RPC V2 contract specification
- `docs/INTERFACE_CHANGELOG.md` - This file
- `docs/MIGRATION_CHECKLIST.md` - Phase-by-phase migration verification
- `docs/REGRESSION_MATRIX.md` - Regression test matrix
