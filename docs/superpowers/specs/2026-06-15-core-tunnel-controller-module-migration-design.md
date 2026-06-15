# Core Tunnel Controller Staged Module Migration Design

Date: 2026-06-15

## Boundary Model

`src/core/tunnel_controller` owns the user-intent driven connection state
machine: phases, events, disconnect reasons, status snapshots, reconnect
policy, heartbeat scheduling, error mapping, engine event bridging, and the
session runner that adapts the native VPN engine to controller events.

`src/platform` owns OS network, route, DNS, tunnel-device, process, helper
transport, and service integration. `src/helper` owns helper protocol and
privileged helper runtime. `src/vpn_engine` owns authentication, CSTP, packet
loop, and native data-plane behavior.

The dependency rule is: tunnel controller may depend on stable platform,
helper, and vpn-engine interfaces; those subsystems must not depend on
controller private runtime details.

## Contract Kernel

The system manifest will contain a `modules.tunnel_controller` entry with:

- boundary accepts/rejects/emits text, matching config/helper style
- ordered phase names and their wire names
- phase runtime traits used by status serialization
- ordered event names
- disconnect reasons
- error domains

Generated artifacts must expose these values to both C++ and TypeScript.
The first phase treats this as an internal contract, not as a new external RPC
surface.

## Module Strategy

The first named module is narrow:

- module name: `exv.core.tunnel.contract`
- exports only simple read-only helpers for contract values
- avoids exporting standard library types through the current MinGW/GCC module
  path
- does not export `TunnelController`, JSON serialization, threading, helper
  clients, platform network operations, or native engine objects

Later phases introduce internal component modules and finally a public
`exv.core.tunnel` module after the runtime implementation has been split out of
`*.inc.cpp` files.

## Implementation Constraints

- Preserve current runtime behavior.
- Preserve existing public include shims until the public module phase.
- Do not add production mocks or stubs.
- Test fakes remain under `tests/`.
- Boundary tests must prevent platform/helper code from depending on
  controller private runtime headers.

## Acceptance Criteria

- Contract generation and snapshot checks include tunnel controller data.
- A focused C++ test proves generated tunnel data matches existing public
  enums.
- App API, core process, and VPN RPC status serialization can use a shared
  tunnel phase contract helper in later phases.
- No phase changes, event changes, or reconnect behavior changes are introduced
  by Phase 1.
