# Architecture Target State

This document defines the target architecture for the ECNU-VPN client, clarifying the role of legacy components and the direction of the native engine.

## Supervisor: Legacy-Only

The VPN supervisor process (`__vpn-supervisor`) is a **legacy component** that only serves the `legacy_openconnect` engine. It is not part of the target architecture.

**Key points:**

- The supervisor (`run_supervisor()` / `run_native_supervisor()` in `src/vpn.cpp`) manages VPN session lifecycle for the legacy openconnect-based path.
- The native engine replaces this with `TunnelController`, which runs inside the Core process (Core-owned mode).
- New features, retry logic, and session management should target the Core process / TunnelController path, not the supervisor.
- The supervisor code is retained for backward compatibility with the legacy engine and will be removed once the legacy engine is fully deprecated.

## Native Engine: Core-Owned Mode

The native VPN engine uses a long-running Core process that owns the VPN tunnel directly:

- `TunnelController` manages connection lifecycle, retries, and state transitions.
- The Core process holds the tunnel session for its entire lifetime.
- The desktop app communicates with Core via RPC; no separate supervisor process is needed.
- Session state is managed through `native-session-state.json` and Core's internal state machine.

## Guidelines

1. **Do not add new functionality to the supervisor.** All new session management, retry, or diagnostic features should be implemented in the Core process or TunnelController.
2. **Supervisor code is frozen** except for critical bug fixes affecting the legacy engine.
3. **Deprecation timeline** for the legacy engine and supervisor will be announced separately.
